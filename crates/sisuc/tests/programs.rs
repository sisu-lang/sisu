//! End to end: programs that need more than a `tests/run` or `tests/run-fail` case (see
//! `cases.rs`), such as a small stack, an allocation count or a generated source, and the bless
//! rules of the `cases` harness.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::ExitStatusExt;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;

mod support;

const PROGRAMS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/programs");
const RUN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/run");
const MIN: &str = "    let min = -9_223_372_036_854_775_807 - 1";
const MAX: &str = "    let big = 9_223_372_036_854_775_807";

/// Compiles `<dir>/<name>.sisu`, run from `dir`, into `<CARGO_TARGET_TMPDIR>/<name>`, or
/// `<name>-O2` with `o2`.
fn compile_to_tmp(dir: &Path, name: &str, o2: bool) -> PathBuf {
    let exe = Path::new(env!("CARGO_TARGET_TMPDIR")).join(if o2 {
        format!("{name}-O2")
    } else {
        name.to_string()
    });
    support::compile(dir, name, &exe, if o2 { &["-O2"] } else { &[] });
    exe
}

/// Checks that `out` is a panic: exit 101, `stdout`, and exactly the panic line on stderr.
fn assert_panics(out: &Output, stdout: &str, stderr: &str) {
    assert_eq!(out.status.code(), Some(101));
    assert_eq!(String::from_utf8_lossy(&out.stdout), stdout);
    assert_eq!(String::from_utf8_lossy(&out.stderr), stderr);
}

/// The programs the small-stack tests run. They are not `tests/run` cases: a million objects
/// under Valgrind is too slow, and `deepchain` overflows on purpose.
const SMALL_STACK: [&str; 8] = [
    "longlist",
    "rightskew",
    "deepchain",
    "eqlist",
    "enumlist",
    "enumfold",
    "enummutual",
    "enumeq",
];

/// `tests/programs` holds exactly the `SMALL_STACK` programs; every other program is a case in
/// `tests/run` or `tests/run-fail`.
#[test]
fn programs_dir_holds_only_small_stack_programs() {
    let mut files: Vec<String> = fs::read_dir(PROGRAMS)
        .expect("tests/programs should be readable")
        .map(|entry| entry.expect("a directory entry should be readable").path())
        // Tools may leave a directory here, such as `.claude/`; only files are programs.
        .filter(|path| !path.is_dir())
        .map(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .expect("a file name should be UTF-8")
                .to_string()
        })
        .collect();
    files.sort();
    let mut expected = SMALL_STACK.map(|name| format!("{name}.sisu"));
    expected.sort();
    assert_eq!(files, expected);
}

/// The `N` of the `total heap usage: N allocs` line in Valgrind's stderr, thousands separators
/// removed; panics when the line is missing.
fn allocs_in(valgrind_stderr: &str) -> u64 {
    let (_, after) = valgrind_stderr
        .split_once("total heap usage: ")
        .unwrap_or_else(|| panic!("no `total heap usage` line in:\n{valgrind_stderr}"));
    let digits: String = after
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',')
        .filter(|c| *c != ',')
        .collect();
    digits
        .parse()
        .unwrap_or_else(|_| panic!("no allocation count after `total heap usage: ` in:\n{after}"))
}

/// Runs `exe` under Valgrind and returns its stdout and `allocs_in` of its stderr. Valgrind runs
/// without `-q`, which prints the heap summary.
fn heap_allocs(exe: &Path) -> (String, u64) {
    let out = support::Invocation::default().run(exe, true);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{}: {stderr}", exe.display());
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        allocs_in(&stderr),
    )
}

/// Builds `<name>.sisu` as written and with its first line `fn iterations() -> i64 { 1000 }`
/// changed to `{ 0 }`, at `-O2` if `o2`, and returns both builds' stdout and the difference in
/// allocations. The zero-iteration build is the baseline: the runtime's own allocations cancel.
fn extra_allocs(name: &str, o2: bool) -> (String, String, i64) {
    const FIRST_LINE: &str = "fn iterations() -> i64 { 1000 }\n";
    let source =
        fs::read_to_string(Path::new(RUN).join(format!("{name}.sisu"))).expect("reads the program");
    let rest = source
        .strip_prefix(FIRST_LINE)
        .unwrap_or_else(|| panic!("{name}.sisu should start with `{}`", FIRST_LINE.trim_end()));
    let baseline = format!("{}{rest}", FIRST_LINE.replace("1000", "0"));

    // Own file names, so these builds never share an executable with the test of `<name>.out`.
    let (stdout, allocs) = heap_allocs(&compile_tmp(&format!("{name}_k1000"), &source, o2));
    let (base_stdout, base_allocs) =
        heap_allocs(&compile_tmp(&format!("{name}_k0"), &baseline, o2));
    let extra = i64::try_from(allocs).expect("a count fits i64")
        - i64::try_from(base_allocs).expect("a count fits i64");
    (stdout, base_stdout, extra)
}

#[test]
fn allocs_in_reads_grouped_counts() {
    let stderr = "==1== total heap usage: 1,004 allocs, 1,004 frees, 9 bytes allocated";
    assert_eq!(allocs_in(stderr), 1004);
}

#[test]
#[should_panic(expected = "no `total heap usage` line")]
fn allocs_in_panics_without_the_summary_line() {
    allocs_in("==1== All heap blocks were freed -- no leaks are possible");
}

/// One test per row, `allocation_counts::<program>`: (program, stdout at k = 1000, stdout at
/// k = 0, extra allocations), checked against `extra_allocs` at both optimization levels.
macro_rules! allocation_counts {
    ($(($name:ident, $stdout:expr, $base_stdout:expr, $extra:expr),)*) => {
        mod allocation_counts {
            $(
                #[test]
                fn $name() {
                    for o2 in [false, true] {
                        assert_eq!(
                            super::extra_allocs(stringify!($name), o2),
                            ($stdout.to_string(), $base_stdout.to_string(), $extra),
                            "o2: {o2}"
                        );
                    }
                }
            )*
        }
    };
}

// The `ALLOCATIONS` table.
allocation_counts! {
    // `big = fill(big, i, i % 2 + 1)` writes in place.
    (handover, "999\n", "0\n", 0),
    // `saved` is live across each write, so each `fill` copies once.
    (copies, "499500\n1000\n", "0\n0\n", 1000),
    // `for` over an array copies nothing.
    (walk, "10000000\n", "0\n", 0),
    // A literal lives in the program image: evaluating it allocates nothing.
    (literal, "3000\n", "0\n", 0),
    // One copy of the literal to capacity 2, then reallocations to 4, 8, ..., 1024. Valgrind
    // counts each `realloc` as an allocation.
    (append_local, "1001\nx\n", "1\na\n", 10),
    (append_assign, "1001\nx\n", "1\na\n", 10),
    // `saved` is live across each append, so each append copies once.
    (append_shared, "1001\n1000\nx\nx\n", "1\n1\na\na\n", 1000),
    (append_grow, "1001\nx\n", "1\na\n", 10),
    // A field or element append is in place too: one copy, then a reallocation per doubling.
    (append_field, "1001\nx\n", "1\na\n", 10),
    (append_field_assign, "1001\nx\n", "1\na\n", 10),
    (append_element, "1001\nx\n", "1\na\n", 10),
    // Each interpolation allocates its result and nothing else.
    (interp, "9\ne\n", "1\n-\n", 1000),
    // Formatting and padding write into that result too: no buffer of their own.
    (interp_format, "    2.67| 日本  |7\n", "-\n", 1000),
    // An enum is held inline: building one whose payloads hold no object allocates nothing.
    (enum_alloc_plain, "1000\n", "0\n", 0),
    // Copying an enum retains its string payload or its boxes and never copies them.
    (enum_alloc_copy, "1000\ntrue\ntrue\n", "0\ntrue\ntrue\n", 0),
    // `Add(Num(1), Num(2))` allocates one box for each of its two fields, and nothing else.
    (enum_alloc_boxes, "1000\n", "0\n", 2000),
    // `match` hands the payload of a scrutinee in its last use to its binding: no array copy.
    (enum_alloc_handover, "1000\n", "0\n", 0),
    // The same through a box with count one: one new box per step, no array copy. Reusing the
    // old box in `match` would make it 0.
    (enum_alloc_handover_box, "1000\n", "0\n", 1000),
    // Through a shared box: one new box and one array copy per step.
    (enum_alloc_shared_box, "1000\n499500\n", "0\n0\n", 2000),
}

/// Compiles `tests/programs/<name>.sisu` without `-O2` and runs it under a 256 KiB stack.
fn run_in_small_stack(name: &str) -> Output {
    run_in_small_stack_at(name, false)
}

/// `run_in_small_stack` with `-O2` if `o2`.
fn run_in_small_stack_at(name: &str, o2: bool) -> Output {
    let exe = compile_to_tmp(Path::new(PROGRAMS), name, o2);
    Command::new("sh")
        .args(["-c", r#"ulimit -c 0 && ulimit -s 256 && exec "$0""#])
        .arg(exe)
        .output()
        .expect("sh starts")
}

/// Runs `<name>.sisu` under the small stack and checks that it exits 0 and prints `expected`.
fn assert_small_stack_prints(name: &str, expected: &str) {
    let out = run_in_small_stack(name);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{:?}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

#[test]
fn longlist_frees_in_constant_stack() {
    assert_small_stack_prints("longlist", "1000000\n");
}

#[test]
fn right_skewed_tree_frees_in_constant_stack() {
    assert_small_stack_prints("rightskew", "1000000\n");
}

/// `==` on two lists of 1 000 000 nodes, with the chain field last and first, at both levels.
#[test]
fn equal_long_lists_compare_in_constant_stack() {
    for o2 in [false, true] {
        let out = run_in_small_stack_at("eqlist", o2);
        assert_eq!(out.status.code(), Some(0), "-O2 {o2}: {:?}", out.status);
        assert_eq!(String::from_utf8_lossy(&out.stdout), "true\ntrue\n");
    }
}

/// `assert_small_stack_prints` at both levels: `-O2` must not need more stack either.
fn assert_small_stack_prints_at_both(name: &str, expected: &str) {
    for o2 in [false, true] {
        let out = run_in_small_stack_at(name, o2);
        assert_eq!(out.status.code(), Some(0), "-O2 {o2}: {:?}", out.status);
        assert_eq!(String::from_utf8_lossy(&out.stdout), expected, "-O2 {o2}");
    }
}

/// A right-deep enum list of 1 000 000 cells frees its boxes from the pending list.
#[test]
fn enum_list_frees_in_constant_stack() {
    assert_small_stack_prints_at_both("enumlist", "1000000\n");
}

/// A left-deep fold of 1 000 000 `Add`s frees its boxes from the pending list.
#[test]
fn enum_fold_frees_in_constant_stack() {
    assert_small_stack_prints_at_both("enumfold", "1000000\n");
}

/// Two chains through two enums that hold each other compare with `==` and free in one frame.
#[test]
fn mutual_enums_compare_and_free_in_constant_stack() {
    assert_small_stack_prints_at_both("enummutual", "1000000\ntrue\n");
}

/// `==` on two pairs of 1 000 000-cell enum lists loops on the tail.
#[test]
fn equal_enum_lists_compare_in_constant_stack() {
    assert_small_stack_prints_at_both("enumeq", "true\nfalse\n");
}

/// The control for `longlist_frees_in_constant_stack`: a drop that recurses once per object
/// overflows the same stack, so the limit is in force.
#[test]
fn deepchain_overflows_the_same_stack() {
    let out = run_in_small_stack("deepchain");
    assert_eq!(out.status.signal(), Some(11), "{:?}", out.status);
    // It dies in the drop, after `print`, but stdout is a pipe and so block-buffered: the
    // runtime flushes only on a normal exit or a Sisu panic, so a signal loses the line, as
    // it does in a C program.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
}

/// Writes `source` to `<CARGO_TARGET_TMPDIR>/<name>.sisu` and compiles it from there, at
/// `-O2` if `o2`.
fn compile_tmp(name: &str, source: &str, o2: bool) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join(format!("{name}.sisu")), source).expect("writes");
    compile_to_tmp(dir, name, o2)
}

/// One test per row, `panic_cases::<name>`: `name => (source, position and message)`. The program
/// `source`, built as `<name>.sisu` at both optimization levels, prints nothing and panics there.
macro_rules! panic_cases {
    ($($name:ident => $case:expr,)*) => {
        mod panic_cases {
            use super::*;
            $(
                #[test]
                fn $name() {
                    let name = stringify!($name);
                    let (source, position_and_message) = $case;
                    for o2 in [false, true] {
                        let out = Command::new(compile_tmp(name, &source, o2))
                            .output()
                            .expect("the program starts");
                        assert_panics(
                            &out,
                            "",
                            &format!("sisu: panic at {name}.sisu:{position_and_message}\n"),
                        );
                    }
                }
            )*
        }
    };
}

panic_cases! {
    sub_overflow => (
        format!("fn main() {{\n{MIN}\n    print(min - 1)\n}}\n"),
        "3:11: integer overflow",
    ),
    mul_overflow => (
        format!("fn main() {{\n{MAX}\n    print(big * 2)\n}}\n"),
        "3:11: integer overflow",
    ),
    neg_overflow => (
        format!("fn main() {{\n{MIN}\n    print(-min)\n}}\n"),
        "3:11: integer overflow",
    ),
    rem_zero => (
        "fn main() {\n    let z = 0\n    print(7 % z)\n}\n",
        "3:11: division by zero",
    ),
    rem_overflow => (
        format!("fn main() {{\n{MIN}\n    print(min % -1)\n}}\n"),
        "3:11: integer overflow",
    ),
    compound_overflow => (
        "fn main() {\n    let mut x = 9_223_372_036_854_775_807\n    x += 1\n    print(x)\n}\n",
        "3:5: integer overflow",
    ),
    u8_add => print_case("let a: u8 = 255", "a + 1", "integer overflow"),
    u8_sub => print_case("let a: u8 = 0", "a - 1", "integer overflow"),
    u8_mul => print_case("let a: u8 = 16", "a * a", "integer overflow"),
    u8_div_zero => print_case("let a: u8 = 0", "7 / a", "division by zero"),
    sum_overflow => sum_case("9_223_372_036_854_775_807, 1"),
    // The total fits, but the running sum overflows at the second element.
    sum_overflow_midway => sum_case("9_223_372_036_854_775_807, 1, -1"),
    bounds_at_length => bounds_case("5", "index 5 out of bounds for length 5"),
    bounds_above => bounds_case("7", "index 7 out of bounds for length 5"),
    bounds_negative => bounds_case("-1", "index -1 out of bounds for length 5"),
    write_bounds => write_bounds_case(),
    append_bounds => append_bounds_case(),
    length_negative => (
        "fn main() {\n    let n = -1\n    let xs = [0; n]\n    print(xs.len())\n}\n",
        "3:18: array length is negative",
    ),
    // 16 + 9_223_372_036_854_775_791 bytes is exactly `i64::MAX`, above the largest
    // size that rounds up to the alignment of 8 within it.
    length_too_large => fill_case("false", "9_223_372_036_854_775_791", 22),
    // The smallest length past `largest_array_reaches_the_allocator`'s: 16 +
    // 9_223_372_036_854_775_785 is `i64::MAX - 6`, which rounds up to 2^63.
    one_too_large => fill_case("false", "9_223_372_036_854_775_785", 22),
    // 2^62 * 8 bytes overflows the multiply.
    length_overflows => fill_case("0", "4_611_686_018_427_387_904", 18),
    // (2^61 - 1) * 8 bytes is 2^64 - 8, which fits; adding the 16-byte header wraps.
    header_overflows => fill_case("0", "2_305_843_009_213_693_951", 18),
}

/// A `panic_cases` row for `grid[0][9] = true`: the inner index is checked at its own level,
/// after the outer one.
fn write_bounds_case() -> (String, String) {
    (
        "fn main() {\n    let mut grid = [[false, false, false]]\n    grid[0][9] = true\n}\n"
            .to_string(),
        "3:13: index 9 out of bounds for length 3".to_string(),
    )
}

/// A `panic_cases` row for `names[i] += say()` with `i` past the end: the append reads the
/// element's slot, bounds-checked, before `say()` runs. `say` would print, so the empty stdout
/// `panic_cases` expects shows the panic comes first.
fn append_bounds_case() -> (String, String) {
    (
        "fn say() -> string {\n    print(\"said\")\n    \"x\"\n}\nfn main() {\n    let mut names = [\"a\", \"b\"]\n    let i = 3\n    names[i] += say()\n}\n"
            .to_string(),
        "8:11: index 3 out of bounds for length 2".to_string(),
    )
}

/// A `panic_cases` row whose panic is at `print(<expr>)` on line 3, column 11.
fn print_case(binding: &str, expr: &str, message: &str) -> (String, String) {
    (
        format!("fn main() {{\n    {binding}\n    print({expr})\n}}\n"),
        format!("3:11: {message}"),
    )
}

/// A `panic_cases` row whose `xs.sum()`, over the elements `elements`, overflows.
fn sum_case(elements: &str) -> (String, String) {
    print_case(
        &format!("let xs = [{elements}]"),
        "xs.sum()",
        "integer overflow",
    )
}

/// A `panic_cases` row for `[value; len]` on line 2, whose panic is at `len`, in column `col`.
fn fill_case(value: &str, len: &str, col: usize) -> (String, String) {
    (
        format!("fn main() {{\n    let xs = [{value}; {len}]\n    print(xs.len())\n}}\n"),
        format!("2:{col}: array is too large"),
    )
}

/// The largest block the size checks allow, `i64::MAX` rounded down to the alignment of 8,
/// passes them and reaches the allocator, which cannot provide it: `sisu_alloc` calls
/// `handle_alloc_error` rather than aborting on an invalid layout, which prints nothing.
#[test]
fn largest_array_reaches_the_allocator() {
    // 16 + 9_223_372_036_854_775_784 bytes is 9_223_372_036_854_775_800.
    let source =
        "fn main() {\n    let xs = [false; 9_223_372_036_854_775_784]\n    print(xs.len())\n}\n";
    for o2 in [false, true] {
        let out = Command::new(compile_tmp("largest_array", source, o2))
            .output()
            .expect("the program starts");
        assert_eq!(out.status.signal(), Some(6), "{:?}", out.status);
        // A backtrace note may follow, depending on `RUST_BACKTRACE`.
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            stderr.lines().next(),
            Some("memory allocation of 9223372036854775800 bytes failed"),
            "{stderr}"
        );
    }
}

/// What the runtime prints when a `print` cannot write to standard output.
const STDOUT_WRITE_PANIC: &str =
    "sisu: panic: standard output could not be written: No space left on device (os error 28)\n";

/// A failed write in `print` of an `i64` or a `bool` is a Sisu panic: one stderr line, exit
/// code 101, no Rust panic text and no abort.
#[test]
fn print_to_a_full_stdout_panics_with_exit_101() {
    for (name, arg) in [("print_full_int", "1"), ("print_full_bool", "true")] {
        let source = format!("fn main() {{\n    print({arg})\n}}\n");
        for o2 in [false, true] {
            let full = File::options()
                .write(true)
                .open("/dev/full")
                .expect("/dev/full exists on Linux");
            let out = Command::new(compile_tmp(name, &source, o2))
                .stdout(full)
                .output()
                .expect("the program starts");
            assert_eq!(out.status.code(), Some(101), "{name}: {:?}", out.status);
            assert_eq!(String::from_utf8_lossy(&out.stderr), STDOUT_WRITE_PANIC);
        }
    }
}

/// A pipe whose read end is closed: a write to the other end raises SIGPIPE.
fn broken_pipe() -> std::io::PipeWriter {
    let (reader, writer) = std::io::pipe().expect("creates a pipe");
    drop(reader);
    writer
}

/// A Sisu panic whose stderr has no reader still exits 101: the runtime ignores SIGPIPE
/// before it writes the panic line. This runs a compiled program, because the libtest child
/// of a runtime unit test starts with SIGPIPE ignored already.
#[test]
fn panic_exits_101_when_stderr_is_a_closed_pipe() {
    let source = "fn main() {\n    let xs = [1, 2]\n    let i = 2\n    print(xs[i])\n}\n";
    for o2 in [false, true] {
        let status = Command::new(compile_tmp("panic_closed_stderr", source, o2))
            .stderr(broken_pipe())
            .status()
            .expect("the program starts");
        assert_eq!(status.code(), Some(101), "{status:?}");
    }
}

/// Standard output with no reader keeps the default SIGPIPE action: the program dies by
/// signal 13, as a C program does.
#[test]
fn print_into_a_closed_stdout_pipe_dies_by_sigpipe() {
    let source = "fn main() {\n    print(1)\n}\n";
    for o2 in [false, true] {
        let status = Command::new(compile_tmp("print_closed_stdout", source, o2))
            .stdout(broken_pipe())
            .status()
            .expect("the program starts");
        assert_eq!(status.signal(), Some(13), "{status:?}");
    }
}

/// Runs `exe` with standard output and standard error both writing to one pipe and returns
/// what arrived, in order, with the exit status.
fn run_with_merged_output(exe: &Path) -> (std::process::ExitStatus, String) {
    use std::io::Read;
    let (mut reader, writer) = std::io::pipe().expect("creates a pipe");
    let mut command = Command::new(exe);
    command
        .stdout(writer.try_clone().expect("clones the pipe"))
        .stderr(writer);
    let mut child = command.spawn().expect("the program starts");
    // The command holds the last write ends; drop them so the read ends at EOF.
    drop(command);
    let mut merged = String::new();
    reader.read_to_string(&mut merged).expect("reads the pipe");
    (child.wait().expect("the program ends"), merged)
}

/// Buffered `print` lines come out before the panic line, in order.
#[test]
fn prints_then_a_panic_flush_stdout_before_the_panic_line() {
    let source = "fn main() {\n    let xs = [1, 2]\n    let i = 2\n    print(1)\n    print(true)\n    print(xs[i])\n}\n";
    for o2 in [false, true] {
        let (status, merged) =
            run_with_merged_output(&compile_tmp("prints_then_panic", source, o2));
        assert_eq!(status.code(), Some(101), "{status:?}");
        let mut lines = merged.lines();
        assert_eq!(lines.next(), Some("1"), "{merged}");
        assert_eq!(lines.next(), Some("true"), "{merged}");
        let panic_line = lines.next().expect("a panic line follows");
        assert!(
            panic_line.starts_with("sisu: panic at ")
                && panic_line.ends_with(": index 2 out of bounds for length 2"),
            "{merged}"
        );
        assert_eq!(lines.next(), None, "{merged}");
    }
}

/// A `panic` call flushes the buffered `print` lines before its panic line, as a run-time
/// panic does.
#[test]
fn prints_then_panic_call_flushes_stdout_first() {
    let source = "fn main() {\n    print(1)\n    print(true)\n    panic(\"x\")\n}\n";
    for o2 in [false, true] {
        let (status, merged) =
            run_with_merged_output(&compile_tmp("prints_then_panic_call", source, o2));
        assert_eq!(status.code(), Some(101), "{status:?}");
        assert_eq!(
            merged, "1\ntrue\nsisu: panic at prints_then_panic_call.sisu:4:5: x\n",
            "{merged}"
        );
    }
}

/// Output is buffered: only the final flush at the end of `main` fails, and it ends the
/// program with the one panic line, not a second error when the process exits.
#[test]
fn prints_then_a_normal_exit_into_a_full_stdout_panics_once() {
    let source = "fn main() {\n    print(1)\n    print(false)\n    print(3)\n}\n";
    for o2 in [false, true] {
        let full = File::options()
            .write(true)
            .open("/dev/full")
            .expect("/dev/full exists on Linux");
        let out = Command::new(compile_tmp("prints_then_full", source, o2))
            .stdout(full)
            .output()
            .expect("the program starts");
        assert_eq!(out.status.code(), Some(101), "{:?}", out.status);
        assert_eq!(String::from_utf8_lossy(&out.stderr), STDOUT_WRITE_PANIC);
    }
}

/// A closed standard output drops what `print` writes and the program exits 0, as with
/// `println!`.
#[test]
fn print_to_a_closed_stdout_is_dropped() {
    let source = "fn main() {\n    print(1)\n}\n";
    for o2 in [false, true] {
        let out = Command::new("sh")
            .args(["-c", r#"exec "$0" >&-"#])
            .arg(compile_tmp("print_closed_fd", source, o2))
            .output()
            .expect("sh starts");
        assert_eq!(out.status.code(), Some(0), "{:?}", out.status);
        assert_eq!(out.stderr, b"");
    }
}

/// A closed standard error drops what `eprint` writes and the program exits 0.
#[test]
fn eprint_to_a_closed_stderr_is_dropped() {
    let source = "fn main() {\n    print(1)\n    eprint(2)\n    print(3)\n}\n";
    for o2 in [false, true] {
        let out = Command::new("sh")
            .args(["-c", r#"exec "$0" 2>&-"#])
            .arg(compile_tmp("eprint_closed_fd", source, o2))
            .output()
            .expect("sh starts");
        assert_eq!(out.status.code(), Some(0), "{:?}", out.status);
        assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n3\n");
    }
}

/// A `panic` with a closed standard error still flushes standard output and exits 101.
#[test]
fn panic_with_a_closed_stderr_exits_101() {
    let source = "fn main() {\n    print(1)\n    panic(\"x\")\n}\n";
    for o2 in [false, true] {
        let out = Command::new("sh")
            .args(["-c", r#"exec "$0" 2>&-"#])
            .arg(compile_tmp("panic_closed_fd", source, o2))
            .output()
            .expect("sh starts");
        assert_eq!(out.status.code(), Some(101), "{:?}", out.status);
        assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n");
    }
}

/// Standard error with no reader keeps the default SIGPIPE action: the program dies by
/// signal 13, as it does for `print` on standard output.
#[test]
fn eprint_into_a_closed_stderr_pipe_dies_by_sigpipe() {
    let source = "fn main() {\n    eprint(1)\n}\n";
    for o2 in [false, true] {
        let status = Command::new(compile_tmp("eprint_closed_pipe", source, o2))
            .stderr(broken_pipe())
            .status()
            .expect("the program starts");
        assert_eq!(status.signal(), Some(13), "{status:?}");
    }
}

/// A failed write to standard error (`ENOSPC`) is ignored: the program goes on and exits 0.
#[test]
fn eprint_to_a_full_stderr_exits_0() {
    let source = "fn main() {\n    eprint(1)\n    print(2)\n}\n";
    for o2 in [false, true] {
        let full = File::options()
            .write(true)
            .open("/dev/full")
            .expect("/dev/full exists on Linux");
        let out = Command::new(compile_tmp("eprint_full", source, o2))
            .stderr(full)
            .output()
            .expect("the program starts");
        assert_eq!(out.status.code(), Some(0), "{:?}", out.status);
        assert_eq!(String::from_utf8_lossy(&out.stdout), "2\n");
    }
}

/// `eprint` does not flush standard output first: on one pipe the `eprint` line comes before an
/// earlier `print` line, which waits for the exit.
#[test]
fn eprint_is_not_held_behind_stdout() {
    let source = "fn main() {\n    print(1)\n    eprint(2)\n}\n";
    for o2 in [false, true] {
        let (status, merged) = run_with_merged_output(&compile_tmp("eprint_order", source, o2));
        assert_eq!(status.code(), Some(0), "{status:?}");
        assert_eq!(merged, "2\n1\n");
    }
}

/// Prints each line of its input.
const ECHO: &str =
    "fn main() {\n    while let line = read_line() {\n        print(line)\n    }\n}\n";

/// Lines of `line <i>` of growing width, each ending in `\n`, up to `total` bytes in all.
fn generated_lines(total: usize) -> Vec<u8> {
    let mut input = Vec::new();
    for i in 0.. {
        let line = format!("line {}\n", "x".repeat(i % 100));
        if input.len() + line.len() > total {
            return input;
        }
        input.extend_from_slice(line.as_bytes());
    }
    unreachable!("the range never ends")
}

/// Prints each of its arguments.
const ECHO_ARGS: &str =
    "fn main(args: [string]) {\n    for a in args {\n        print(a)\n    }\n}\n";

/// An invalid byte in a line of input, or in an argument, comes back as the three bytes of
/// `U+FFFD`, which a `.out` file, read through `from_utf8_lossy`, cannot tell from the raw byte.
#[test]
fn invalid_utf8_is_repaired_byte_for_byte() {
    for o2 in [false, true] {
        let exe = compile_tmp("echo_repair", ECHO, o2);
        let out = support::Invocation::new(b"a\xFFb\n".to_vec(), Vec::new()).run(&exe, false);
        assert_eq!(out.status.code(), Some(0), "{:?}", out.status);
        assert_eq!(out.stdout, b"a\xEF\xBF\xBDb\n");
        let exe = compile_tmp("echo_args_repair", ECHO_ARGS, o2);
        let arg = OsStr::from_bytes(b"x\xFFy").to_os_string();
        let out = support::Invocation::new(Vec::new(), vec![arg]).run(&exe, true);
        assert_eq!(out.status.code(), Some(0), "{:?}", out.status);
        assert_eq!(out.stdout, b"x\xEF\xBF\xBDy\n");
    }
}

/// An argument holding a space, which a `// args:` line cannot pass, stays one argument.
#[test]
fn args_keep_spaces() {
    for o2 in [false, true] {
        let exe = compile_tmp("echo_args_spaces", ECHO_ARGS, o2);
        let out = support::Invocation::new(Vec::new(), vec!["two words".into()]).run(&exe, false);
        assert_eq!(out.status.code(), Some(0), "{:?}", out.status);
        assert_eq!(String::from_utf8_lossy(&out.stdout), "two words\n");
    }
}

/// A line longer than the runtime's 8 KiB input buffer, input of several buffers, and a `€`
/// at bytes 8191..8194 (across the first buffer's end when the first read fills it) all come
/// back whole, with no memory error. The `Lines` unit tests force the split itself.
#[test]
fn read_line_refills_its_buffer() {
    let mut first = "a".repeat(8191) + "€";
    first += &"b".repeat(20_000 - first.len());
    let mut input = first.into_bytes();
    input.push(b'\n');
    input.extend(generated_lines(40_000 - input.len()));
    assert_eq!(&input[8191..8194], "€".as_bytes());
    for o2 in [false, true] {
        let exe = compile_tmp("echo_refill", ECHO, o2);
        let out = support::Invocation::new(input.clone(), Vec::new()).run(&exe, true);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{stderr}");
        assert!(out.stdout == input, "{stderr}");
    }
}

/// A line of 2 MiB holds a million words, each found from where the last one ended. A split
/// that rescanned from the start of the line would take far longer than the test's timeout.
/// Both forms run: a `[string]` kept in a local is built word by word, and
/// `line.words().len()` only counts.
#[test]
fn words_is_linear() {
    let built = "fn main() {\n    if let line = read_line() {\n        let ws = line.words()\n        print(ws.len())\n    }\n}\n";
    let counted = "fn main() {\n    if let line = read_line() {\n        print(line.words().len())\n    }\n}\n";
    let input = "a ".repeat(1 << 20).into_bytes();
    for (name, source) in [("words_linear", built), ("words_count_linear", counted)] {
        for o2 in [false, true] {
            let exe = compile_tmp(name, source, o2);
            let out = support::Invocation::new(input.clone(), Vec::new()).run(&exe, false);
            assert_eq!(out.status.code(), Some(0), "{name}: {:?}", out.status);
            assert_eq!(String::from_utf8_lossy(&out.stdout), "1048576\n", "{name}");
        }
    }
}

/// More input than a pipe holds reaches the program while the harness reads its output, so
/// neither side waits on the other.
#[test]
fn stdin_larger_than_a_pipe() {
    let input = generated_lines(1 << 20);
    for o2 in [false, true] {
        let exe = compile_tmp("echo_large", ECHO, o2);
        let out = support::Invocation::new(input.clone(), Vec::new()).run(&exe, false);
        assert_eq!(out.status.code(), Some(0), "{:?}", out.status);
        assert!(out.stdout == input, "{} bytes out", out.stdout.len());
    }
}

/// A program that exits before it reads all its input ends normally; the harness ignores
/// the failed write of the rest.
#[test]
fn program_exiting_before_its_input_ends() {
    let source = "fn main() {\n    print(read_line() ?? \"none\")\n}\n";
    let input = generated_lines(1 << 20);
    for o2 in [false, true] {
        let exe = compile_tmp("read_one_line", source, o2);
        let out = support::Invocation::new(input.clone(), Vec::new()).run(&exe, false);
        assert_eq!(out.status.code(), Some(0), "{:?}", out.status);
        assert_eq!(String::from_utf8_lossy(&out.stdout), "line \n");
    }
}

/// A failed read (`EISDIR`) is a panic with no location.
#[test]
fn read_line_from_a_directory_panics() {
    let source = "fn main() {\n    print(read_line() ?? \"none\")\n}\n";
    for o2 in [false, true] {
        let dir = File::open(env!("CARGO_TARGET_TMPDIR")).expect("opens the directory");
        let out = Command::new(compile_tmp("read_directory", source, o2))
            .stdin(Stdio::from(dir))
            .output()
            .expect("the program starts");
        assert_panics(
            &out,
            "",
            "sisu: panic: standard input could not be read: Is a directory (os error 21)\n",
        );
    }
}

/// A closed standard input is the end of input.
#[test]
fn read_line_from_a_closed_stdin_is_none() {
    let source = "fn main() {\n    print(read_line() ?? \"none\")\n}\n";
    for o2 in [false, true] {
        let out = Command::new("sh")
            .args(["-c", r#"exec "$0" <&-"#])
            .arg(compile_tmp("read_closed_fd", source, o2))
            .output()
            .expect("sh starts");
        assert_eq!(out.status.code(), Some(0), "{:?}", out.status);
        assert_eq!(String::from_utf8_lossy(&out.stdout), "none\n");
    }
}

/// An `--rc-checks` program that leaks exits 102 and still delivers what it printed.
#[test]
fn rc_checks_exit_flushes_stdout() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("rc_flush.sisu"), LEAKS_A_CYCLE).expect("writes");
    let exe = dir.join("rc_flush");
    support::compile(dir, "rc_flush", &exe, &["--rc-checks"]);
    let (status, merged) = run_with_merged_output(&exe);
    assert_eq!(status.code(), Some(102), "{status:?}");
    assert_eq!(merged, "1\nsisu: rc check: live objects at exit: 1\n");
}

/// Output goes out in blocks while the program runs: a program that prints 3000 lines and
/// then dies by a stack overflow leaves whole blocks on a pipe and loses only the tail.
#[test]
fn full_blocks_of_output_survive_a_crash() {
    let source = "fn dive(n: i64) -> i64 {\n    dive(n + 1) + 1\n}\nfn main() {\n    let mut i = 0\n    while i < 3000 {\n        print(i)\n        i += 1\n    }\n    print(dive(0))\n}\n";
    let exe = compile_tmp("blocks_then_crash", source, false);
    let out = Command::new("sh")
        .args(["-c", r#"ulimit -c 0 && exec "$0""#])
        .arg(exe)
        .output()
        .expect("sh starts");
    assert_eq!(out.status.signal(), Some(11), "{:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let all: String = (0..3000).map(|i| i.to_string() + "\n").collect();
    assert!(stdout.len() >= 8192, "{} bytes", stdout.len());
    assert!(stdout.len() < all.len(), "the tail is still in the buffer");
    assert!(all.starts_with(&*stdout), "whole lines, in order");
    assert!(stdout.ends_with('\n'), "{stdout:?}");
}

/// `print` collects its output in one buffer, however much the program prints.
#[test]
fn print_allocates_one_buffer() {
    let program = |limit: u32| {
        format!(
            "fn main() {{\n    let mut i = 0\n    while i < {limit} {{\n        print(i)\n        i += 1\n    }}\n}}\n"
        )
    };
    let (_, one) = heap_allocs(&compile_tmp("one_buffer_k1", &program(1), false));
    let (_, many) = heap_allocs(&compile_tmp("one_buffer_k20000", &program(20_000), false));
    assert_eq!(many, one);
}

/// Enough output to fill the buffer many times arrives whole and in order.
#[test]
fn a_long_output_to_a_file_is_complete() {
    let source = "fn main() {\n    let mut i = 0\n    while i < 100_000 {\n        print(i)\n        i += 1\n    }\n}\n";
    let numbers: Vec<String> = (0..100_000).map(|i| i.to_string()).collect();
    let expected = numbers.join("\n") + "\n";
    for o2 in [false, true] {
        let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("long_output.txt");
        let status = Command::new(compile_tmp("long_output", source, o2))
            .stdout(File::create(&path).expect("creates the file"))
            .status()
            .expect("the program starts");
        assert_eq!(status.code(), Some(0), "{status:?}");
        assert_eq!(fs::read_to_string(&path).expect("reads the file"), expected);
    }
}

/// A `panic_cases` row that reads `xs[i]` from a 5-element `xs`, with `i` bound to `index`;
/// the panic is at `i`, on line 4, column 14.
fn bounds_case(index: &str, message: &str) -> (String, String) {
    (
        format!(
            "fn main() {{\n    let xs = [1, 2, 3, 4, 5]\n    let i = {index}\n    print(xs[i])\n}}\n"
        ),
        format!("4:14: {message}"),
    )
}

/// Prints `1` and exits 0, but leaks: an object that refers to itself is never freed. Building
/// the cycle in a callee keeps the pointer out of `main`'s registers.
const LEAKS_A_CYCLE: &str = "class Node {\n    mut next: Node?\n}\n\nfn leak() {\n    let a = Node(next: None)\n    a.next = a\n}\n\nfn main() {\n    leak()\n    print(1)\n}\n";

/// Proves the harness catches a leak.
#[test]
fn valgrind_reports_a_cycle() {
    let exe = compile_tmp("cycle", LEAKS_A_CYCLE, false);
    let out = Command::new(&exe).output().expect("the program starts");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n");
    let checked = support::Invocation::default().run(&exe, true);
    let stderr = String::from_utf8_lossy(&checked.stderr);
    assert_eq!(checked.status.code(), Some(1), "{stderr}");
    // A loss record, not the summary line, which also prints `definitely lost: 0 bytes`.
    assert!(
        stderr.contains("are definitely lost") || stderr.contains("are indirectly lost"),
        "{stderr}"
    );
}

/// Writes `source` to `<CARGO_TARGET_TMPDIR>/<name>.sisu`, builds it with `--rc-checks`, at
/// `-O2` if `o2`, into `<name>-rc` or `<name>-rc-O2`, and runs it.
fn run_rc_checked(name: &str, source: &str, o2: bool) -> Output {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join(format!("{name}.sisu")), source).expect("writes");
    let (exe, flags): (_, &[&str]) = if o2 {
        (format!("{name}-rc-O2"), &["-O2", "--rc-checks"])
    } else {
        (format!("{name}-rc"), &["--rc-checks"])
    };
    let exe = dir.join(exe);
    support::compile(dir, name, &exe, flags);
    Command::new(exe).output().expect("the program starts")
}

/// The cycle Valgrind reports, `--rc-checks` reports too, with or without `-O2`: one object is
/// live at exit.
#[test]
fn rc_checks_report_a_cycle() {
    for o2 in [false, true] {
        let out = run_rc_checked("rc_cycle", LEAKS_A_CYCLE, o2);
        assert_eq!(out.status.code(), Some(102), "o2: {o2}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n");
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            "sisu: rc check: live objects at exit: 1\n"
        );
    }
}

/// A panic exits 101 without releasing, so the object `b` still holds is no leak to report.
#[test]
fn rc_checks_ignore_a_panic() {
    let out = run_rc_checked(
        "rc_panic",
        "class Box {\n    v: i64\n}\n\nfn main() {\n    let b = Box(v: 1)\n    let z = 0\n    print(1 / z)\n    print(b.v)\n}\n",
        false,
    );
    assert_panics(
        &out,
        "",
        "sisu: panic at rc_panic.sisu:8:11: division by zero\n",
    );
}

#[test]
fn panic_exits_101_when_stderr_write_fails() {
    // Writes to /dev/full fail with ENOSPC, so the runtime's panic line cannot be written.
    let full = File::options()
        .write(true)
        .open("/dev/full")
        .expect("Linux has /dev/full, the only target Sisu supports");
    let exe = compile_tmp(
        "stderr_full",
        "fn main() {\n    let z = 0\n    print(1 / z)\n}\n",
        false,
    );
    let status = Command::new(exe)
        .stderr(Stdio::from(full))
        .status()
        .expect("the program starts");
    assert_eq!(status.code(), Some(101));
}

#[test]
fn output_naming_the_input_is_refused() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let source = "fn main() {\n    print(1)\n}\n";
    let input = dir.join("overwrite_input.sisu");
    fs::write(&input, source).expect("writes");
    for output in ["overwrite_input.sisu", "./overwrite_input.sisu"] {
        let out = support::sisuc(dir, "overwrite_input.sisu", Path::new(output), &[])
            .output()
            .expect("sisuc starts");
        assert_eq!(out.status.code(), Some(1), "output {output}");
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            format!("sisuc: output {output} would overwrite the input\n")
        );
        assert_eq!(fs::read_to_string(&input).expect("reads"), source);
    }
}

/// Builds the `tests/run` case `loops` with `flags` and `PATH`, runs it, and checks its output.
/// Returns the executable's bytes.
fn build_loops(exe: &str, flags: &[&str], path: &OsStr) -> Vec<u8> {
    let exe = Path::new(env!("CARGO_TARGET_TMPDIR")).join(exe);
    let out = support::sisuc(Path::new(RUN), "loops.sisu", &exe, flags)
        .env("PATH", path)
        .output()
        .expect("sisuc starts");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&exe).output().expect("the program starts");
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        fs::read_to_string(Path::new(RUN).join("loops.out")).expect("reads the expected output")
    );
    fs::read(&exe).expect("reads the executable")
}

#[test]
fn o3_builds_a_program_that_runs() {
    build_loops(
        "loops-O3",
        &["-O3"],
        &std::env::var_os("PATH").unwrap_or_default(),
    );
}

/// `--linker lld` links with LLVM's `ld.lld`, which records itself in the `.comment` section;
/// the default linker, GNU ld, does not.
#[test]
fn linker_lld_links_with_lld() {
    let bin = Path::new(env!("LLVM_SYS_221_PREFIX")).join("bin");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&path)))
        .expect("PATH joins");
    let lld = build_loops("loops-lld", &["--linker", "lld"], &path);
    assert!(lld.windows(11).any(|w| w == b"Linker: LLD"));
    let default = build_loops("loops-ld", &[], &path);
    assert!(!default.windows(11).any(|w| w == b"Linker: LLD"));
}

#[test]
fn a_missing_linker_is_named() {
    let empty = empty_dir("no_linker_on_path");
    let exe = empty.join("loops");
    let out = support::sisuc(Path::new(RUN), "loops.sisu", &exe, &["--linker", "mold"])
        .env("PATH", &empty)
        .output()
        .expect("sisuc starts");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "sisuc: linker ld.mold not found on PATH\n"
    );
    assert_empty(&empty);
}

/// An empty directory under the target tmp dir, for `sisuc` to use as `TMPDIR`.
fn empty_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    // It is absent on the first run, so a failure here is expected and harmless.
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).expect("creates the directory");
    dir
}

fn assert_empty(dir: &Path) {
    let left: Vec<_> = fs::read_dir(dir).expect("reads").collect();
    assert!(left.is_empty(), "sisuc left {left:?}");
}

#[test]
fn existing_object_file_survives_a_build() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("keep_object.sisu"), "fn main() {}\n").expect("writes");
    fs::write(dir.join("keep_object.o"), "not sisuc's").expect("writes");
    let tmp = empty_dir("keep_object_tmp");
    let out = support::sisuc(dir, "keep_object.sisu", &dir.join("keep_object"), &[])
        .env("TMPDIR", &tmp)
        .output()
        .expect("sisuc starts");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(dir.join("keep_object.o")).expect("`keep_object.o` is still there"),
        "not sisuc's"
    );
    assert_empty(&tmp);
}

#[test]
fn failed_link_leaves_no_object_file() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("link_fails.sisu"), "fn main() {}\n").expect("writes");
    // `cc` cannot write an executable over a directory.
    let output = empty_dir("link_fails");
    let tmp = empty_dir("link_fails_tmp");
    let out = support::sisuc(dir, "link_fails.sisu", &output, &[])
        .env("TMPDIR", &tmp)
        .output()
        .expect("sisuc starts");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("sisuc: cc failed: "), "{stderr}");
    assert_empty(&tmp);
    assert!(!dir.join("link_fails.o").exists());
}

/// Runs `sisuc <name>.sisu <name>` on `source` in the target tmp dir, after removing any
/// earlier `<name>`; returns the output and the executable's path.
fn build_tmp(name: &str, source: &str) -> (Output, PathBuf) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join(format!("{name}.sisu")), source).expect("writes");
    let exe = dir.join(name);
    // It is absent on the first run, so a failure here is expected and harmless.
    let _ = fs::remove_file(&exe);
    let out = support::sisuc(dir, &format!("{name}.sisu"), &exe, &[])
        .output()
        .expect("sisuc starts");
    (out, exe)
}

#[test]
fn build_error_makes_no_executable() {
    let (out, exe) = build_tmp(
        "build_error_makes_no_executable",
        "fn main() {\n    let n = 0\n    n = 1\n}\n",
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("error[E0001]: cannot assign to `n`"));
    assert!(!exe.exists());
}

#[test]
fn build_warning_still_makes_the_executable() {
    let (out, exe) = build_tmp(
        "build_warning_still_makes_the_executable",
        "fn main() {\n    let mut count = 0\n    print(count)\n}\n",
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .starts_with("warning[W0002]: `count` is never reassigned")
    );
    let run = Command::new(exe).output().expect("the program starts");
    assert_eq!(String::from_utf8_lossy(&run.stdout), "0\n");
}

/// A program that prints `1`.
const PRINTS_1: &str = "fn main() {\n    print(1)\n}\n";
/// A program that prints `1`, then panics at 4:11 with `division by zero`. It prints first so
/// that a write of its stdout would change an empty `.out`.
const PRINTS_1_THEN_PANICS: &str =
    "fn main() {\n    print(1)\n    let z = 0\n    print(1 / z)\n}\n";

/// Writes each `(path, contents)` under a fresh `<CARGO_TARGET_TMPDIR>/fixtures/<test>/` and
/// returns that directory.
fn fixture(test: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("fixtures")
        .join(test);
    // It is absent on the first run, so a failure here is expected and harmless.
    let _ = fs::remove_dir_all(&dir);
    for (path, contents) in files {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().expect("a fixture file should have a parent"))
            .expect("creates the fixture directory");
        fs::write(path, contents).expect("writes the fixture file");
    }
    dir
}

/// Runs `f`, which should panic, and returns its panic message.
fn panic_message(f: impl FnOnce()) -> String {
    let payload = panic::catch_unwind(AssertUnwindSafe(f)).expect_err("the call should panic");
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .expect("a panic message should be a string")
}

#[test]
fn bless_refuses_to_create_a_missing_out() {
    let dir = fixture(
        "bless_refuses_to_create_a_missing_out",
        &[("a.sisu", PRINTS_1)],
    );
    let message = panic_message(|| support::run(&dir.join("a.sisu"), true));
    assert!(message.contains("is missing; create it empty"), "{message}");
    assert!(!dir.join("a.out").exists());
}

#[test]
fn bless_never_writes_after_a_wrong_exit() {
    let dir = fixture(
        "bless_never_writes_after_a_wrong_exit",
        &[("a.sisu", PRINTS_1_THEN_PANICS), ("a.out", "")],
    );
    let message = panic_message(|| support::run(&dir.join("a.sisu"), true));
    assert!(message.contains("Some(101)"), "{message}");
    assert_eq!(
        fs::read_to_string(dir.join("a.out")).expect("reads a.out"),
        ""
    );
}

/// An `o2-calls` marker fails the case when the inliner folds the call away.
#[test]
fn o2_calls_marker_fails_on_an_inlined_call() {
    let dir = fixture(
        "o2_calls_marker_fails_on_an_inlined_call",
        &[
            (
                "a.sisu",
                "// o2-calls: main one 1\nfn one() -> i64 { 1 }\n\nfn main() {\n    print(one())\n}\n",
            ),
            ("a.out", "1\n"),
        ],
    );
    let message = panic_message(|| support::run(&dir.join("a.sisu"), false));
    assert!(
        message.contains("-O2 keeps 0 calls to `one` in `main`, not 1"),
        "{message}"
    );
}

const DOWN: &str = "fn down(n: i64) -> i64 {\n    if n > 0 { down(n - 1) + 1 } else { 0 }\n}\n\n";

/// `main` calls `outer` twice, so `-O2` inlines it into `main` twice; in each copy `down(0)` has
/// folded to a constant and `down(5)` keeps its call.
const OUTER_CALLS_DOWN: &str = "fn outer() -> i64 {\n    down(0) + down(5)\n}\n\n\
     fn main() {\n    print(outer())\n    print(outer())\n}\n";

/// `fixture` with the case `// o2-calls: <markers>` and `DOWN`, `body`, whose run must print `out`.
fn o2_case(test: &str, markers: &str, body: &str, out: &str) -> PathBuf {
    fixture(
        test,
        &[
            ("a.sisu", &format!("// o2-calls: {markers}\n{DOWN}{body}")),
            ("a.out", out),
        ],
    )
}

/// A marker counts the calls in the caller's own body: `outer` keeps one call to `down`, not
/// the two that its inlined copies in `main` add up to.
#[test]
fn o2_calls_marker_counts_inside_the_caller() {
    let dir = o2_case(
        "o2_calls_marker_counts_inside_the_caller_1",
        "outer down 1",
        OUTER_CALLS_DOWN,
        "5\n5\n",
    );
    support::run(&dir.join("a.sisu"), false);
    let dir = o2_case(
        "o2_calls_marker_counts_inside_the_caller_2",
        "outer down 2",
        OUTER_CALLS_DOWN,
        "5\n5\n",
    );
    let message = panic_message(|| support::run(&dir.join("a.sisu"), false));
    assert!(
        message.contains("-O2 keeps 1 calls to `down` in `outer`, not 2"),
        "{message}"
    );
}

/// Each caller gets a build of its own. Keeping `outer` for its marker must not keep it for
/// `main`'s: a real build inlines `outer`, so `main` keeps no call to it.
#[test]
fn o2_calls_marker_keeps_one_caller_at_a_time() {
    let dir = o2_case(
        "o2_calls_marker_keeps_one_caller_at_a_time",
        "main outer 2\n// o2-calls: outer down 1",
        OUTER_CALLS_DOWN,
        "5\n5\n",
    );
    let message = panic_message(|| support::run(&dir.join("a.sisu"), false));
    assert!(
        message.contains("-O2 keeps 0 calls to `outer` in `main`, not 2"),
        "{message}"
    );
}

/// With `outer` kept as a function of its own, `outer(n)` holds a call to `down` that its real
/// call sites decide away, so a caller with parameters is rejected.
#[test]
fn o2_calls_marker_rejects_a_caller_with_parameters() {
    let dir = o2_case(
        "o2_calls_marker_rejects_a_caller_with_parameters",
        "outer down 1",
        "fn outer(n: i64) -> i64 {\n    if n == 5 { down(n) } else { n }\n}\n\n\
         fn main() {\n    print(outer(1))\n    print(outer(10))\n}\n",
        "1\n10\n",
    );
    let message = panic_message(|| support::run(&dir.join("a.sisu"), false));
    assert!(
        message.contains("o2-calls caller `outer` takes parameters"),
        "{message}"
    );
}

/// `down(0)` folds to a constant, so a count of 0 holds and proves nothing; it is rejected.
#[test]
fn o2_calls_marker_rejects_a_count_of_zero() {
    let dir = o2_case(
        "o2_calls_marker_rejects_a_count_of_zero",
        "main down 0",
        "fn main() {\n    print(down(0))\n}\n",
        "0\n",
    );
    let message = panic_message(|| support::run(&dir.join("a.sisu"), false));
    assert!(message.contains("proves no call boundary"), "{message}");
}

/// A case has one argument list, so a second `// args:` line is a mistake.
#[test]
#[should_panic(expected = "more than one `// args:` line")]
fn two_args_markers_fail() {
    let dir = fixture(
        "two_args_markers_fail",
        &[("a.sisu", "// args: a\n// args: b\nfn main() {}\n")],
    );
    support::Invocation::of(&dir.join("a.sisu"));
}

#[test]
fn o2_calls_marker_names_a_missing_function() {
    for (test, marker, missing) in [
        (
            "o2_calls_marker_names_a_missing_caller",
            "outr down 1",
            "outr",
        ),
        (
            "o2_calls_marker_names_a_missing_callee",
            "outer dwn 1",
            "dwn",
        ),
    ] {
        let dir = o2_case(test, marker, OUTER_CALLS_DOWN, "5\n5\n");
        let message = panic_message(|| support::run(&dir.join("a.sisu"), false));
        assert!(
            message.contains(&format!("no function named `{missing}`")),
            "{message}"
        );
    }
}

#[test]
fn o2_calls_marker_must_name_caller_callee_and_count() {
    let dir = fixture(
        "o2_calls_marker_must_name_caller_callee_and_count",
        &[
            (
                "a.sisu",
                "// o2-calls: main one\nfn main() {\n    print(1)\n}\n",
            ),
            ("a.out", "1\n"),
        ],
    );
    let message = panic_message(|| support::run(&dir.join("a.sisu"), false));
    assert!(
        message.contains("is not `<caller> <callee> <count>`"),
        "{message}"
    );
}

/// A run that passes natively but fails under Valgrind writes nothing.
#[test]
fn bless_never_writes_after_a_valgrind_failure() {
    let dir = fixture(
        "bless_never_writes_after_a_valgrind_failure",
        &[("a.sisu", LEAKS_A_CYCLE), ("a.out", "wrong\n")],
    );
    let message = panic_message(|| support::run(&dir.join("a.sisu"), true));
    assert!(message.contains("LEAK SUMMARY"), "{message}");
    assert_eq!(
        fs::read_to_string(dir.join("a.out")).expect("reads a.out"),
        "wrong\n"
    );
}

/// Bless writes a `run_fail` case's files all or nothing: a missing `.err` leaves `.out` as it was.
#[test]
fn bless_writes_neither_file_when_one_is_missing() {
    let dir = fixture(
        "bless_writes_neither_file_when_one_is_missing",
        &[("a.sisu", PRINTS_1_THEN_PANICS), ("a.out", "")],
    );
    let message = panic_message(|| support::run_fail(&dir.join("a.sisu"), true));
    assert!(message.contains("a.err is missing"), "{message}");
    assert_eq!(
        fs::read_to_string(dir.join("a.out")).expect("reads a.out"),
        ""
    );
    assert!(!dir.join("a.err").exists());
}

/// A program that reports nothing is no diagnostics case, whatever its `.err` holds.
#[test]
fn diagnostics_rejects_a_clean_program() {
    let dir = fixture(
        "diagnostics_rejects_a_clean_program",
        &[("a.sisu", "fn main() {}\n"), ("a.err", "")],
    );
    let message = panic_message(|| support::diagnostics(&dir.join("a.sisu"), true));
    assert!(message.contains("reports no diagnostic"), "{message}");
    assert_eq!(
        fs::read_to_string(dir.join("a.err")).expect("reads a.err"),
        ""
    );
}

/// Without bless a wrong `.out` fails and stays as it was; bless rewrites it, and the run then
/// passes without bless.
#[test]
fn bless_rewrites_an_existing_out() {
    let dir = fixture(
        "bless_rewrites_an_existing_out",
        &[("a.sisu", PRINTS_1), ("a.out", "wrong\n")],
    );
    let source = dir.join("a.sisu");
    let message = panic_message(|| support::run(&source, false));
    assert!(message.contains("SISU_BLESS=1"), "{message}");
    assert_eq!(
        fs::read_to_string(dir.join("a.out")).expect("reads a.out"),
        "wrong\n"
    );
    support::run(&source, true);
    assert_eq!(
        fs::read_to_string(dir.join("a.out")).expect("reads a.out"),
        "1\n"
    );
    support::run(&source, false);
}

/// A program that prints `1` and writes `2` to standard error.
const PRINTS_1_EPRINTS_2: &str = "fn main() {\n    print(1)\n    eprint(2)\n}\n";

/// A `run` case that writes to standard error needs its `.err`: a wrong one fails and stays as it
/// was, bless rewrites it, and the run then passes. Without one the stderr must be empty.
#[test]
fn run_err_lifecycle() {
    let dir = fixture(
        "run_err_lifecycle",
        &[
            ("a.sisu", PRINTS_1_EPRINTS_2),
            ("a.out", "1\n"),
            ("a.err", "wrong\n"),
            ("b.sisu", PRINTS_1_EPRINTS_2),
            ("b.out", "1\n"),
        ],
    );
    let source = dir.join("a.sisu");
    let message = panic_message(|| support::run(&source, false));
    assert!(message.contains("SISU_BLESS=1"), "{message}");
    assert_eq!(
        fs::read_to_string(dir.join("a.err")).expect("reads a.err"),
        "wrong\n"
    );
    support::run(&source, true);
    assert_eq!(
        fs::read_to_string(dir.join("a.err")).expect("reads a.err"),
        "2\n"
    );
    support::run(&source, false);
    let message = panic_message(|| support::run(&dir.join("b.sisu"), false));
    assert!(message.contains("has no `.err` file"), "{message}");
}

#[test]
#[should_panic(expected = "ghost.out")]
fn orphan_out_fails() {
    let dir = fixture("orphan_out_fails", &[("ghost.out", "1\n")]);
    support::orphan_check(&dir.join("ghost.out"), &["out"]);
}

#[test]
#[should_panic(expected = "notes.txt")]
fn stray_file_type_fails() {
    let dir = fixture(
        "stray_file_type_fails",
        &[("notes.sisu", PRINTS_1), ("notes.txt", "")],
    );
    support::orphan_check(&dir.join("notes.txt"), &support::RUN_EXPECTATIONS);
}

/// A `.json` beside a `run` case is never read, so it fails like an orphan.
#[test]
#[should_panic(expected = "a.json")]
fn expectation_of_another_mode_fails() {
    let dir = fixture(
        "expectation_of_another_mode_fails",
        &[("a.sisu", PRINTS_1), ("a.out", "1\n"), ("a.json", "")],
    );
    support::orphan_check(&dir.join("a.json"), &support::RUN_EXPECTATIONS);
}

/// The harness never lists a hidden file, so the check of a listed file beside it fails.
#[test]
#[should_panic(expected = ".ghost.out")]
fn hidden_file_fails() {
    let dir = fixture(
        "hidden_file_fails",
        &[
            ("a.sisu", PRINTS_1),
            ("a.out", "1\n"),
            (".ghost.out", "1\n"),
        ],
    );
    support::orphan_check(&dir.join("a.sisu"), &["out"]);
}

/// Same-named cases in different directories, run at once, must not share an executable: one
/// in each mode, and two whose paths differ only in `a_b` against `a/b`.
#[test]
fn same_name_in_two_modes() {
    let dir = fixture(
        "same_name_in_two_modes",
        &[
            ("run/dup.sisu", PRINTS_1),
            ("run/dup.out", "1\n"),
            ("run/a_b/dup.sisu", PRINTS_1),
            ("run/a_b/dup.out", "1\n"),
            ("run/a/b/dup.sisu", PRINTS_1_THEN_PANICS),
            ("run/a/b/dup.out", "1\n"),
            (
                "run/a/b/dup.err",
                "sisu: panic at dup.sisu:4:11: division by zero\n",
            ),
            ("run-fail/dup.sisu", PRINTS_1_THEN_PANICS),
            ("run-fail/dup.out", "1\n"),
            (
                "run-fail/dup.err",
                "sisu: panic at dup.sisu:4:11: division by zero\n",
            ),
        ],
    );
    thread::scope(|scope| {
        scope.spawn(|| support::run(&dir.join("run/dup.sisu"), false));
        scope.spawn(|| support::run_fail(&dir.join("run-fail/dup.sisu"), false));
        scope.spawn(|| support::run(&dir.join("run/a_b/dup.sisu"), false));
        scope.spawn(|| support::run_fail(&dir.join("run/a/b/dup.sisu"), false));
    });
}

/// Each program builds to the same bytes twice, at two different output paths.
#[test]
fn builds_are_deterministic() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    for name in ["life", "optionals", "arrays"] {
        let [a, b] = ["a", "b"].map(|build| {
            let exe = dir.join(format!("deterministic-{name}-{build}"));
            support::compile(Path::new(RUN), name, &exe, &["-O2"]);
            fs::read(&exe).expect("reads the build")
        });
        assert!(a == b, "{name}.sisu: two -O2 builds differ");
    }
}

/// Generated `i64`, `u8` and `bool` expressions, which Sisu at O0 and `-O2` must print as Rust
/// computes them.
mod arithmetic {
    use std::fmt;
    use std::process::Command;

    use super::compile_tmp;

    /// Xorshift64: fixed seeds give the same expressions on every run.
    struct XorShift(u64);

    impl XorShift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn pick<T: Copy>(&mut self, items: &[T]) -> T {
            let len = u64::try_from(items.len()).expect("a slice length fits u64");
            items[usize::try_from(self.next() % len).expect("an index below a length fits usize")]
        }
    }

    #[derive(Clone, Copy)]
    enum Ty {
        I64,
        U8,
        Bool,
    }

    #[derive(Clone, Copy)]
    enum Value {
        I64(i64),
        U8(u8),
        Bool(bool),
    }

    impl Value {
        /// An integer as `i64`, which holds every `u8` and keeps its order.
        fn int(self) -> i64 {
            match self {
                Value::I64(n) => n,
                Value::U8(n) => n.into(),
                Value::Bool(_) => panic!("a generated integer operand is a bool"),
            }
        }

        fn truth(self) -> bool {
            match self {
                Value::Bool(b) => b,
                _ => panic!("a generated condition is not a bool"),
            }
        }
    }

    impl fmt::Display for Value {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Value::I64(n) => write!(f, "{n}"),
                Value::U8(n) => write!(f, "{n}"),
                Value::Bool(b) => write!(f, "{b}"),
            }
        }
    }

    const ARITH: [&str; 5] = ["+", "-", "*", "/", "%"];
    const COMPARE: [&str; 6] = ["==", "!=", "<", "<=", ">", ">="];
    /// Literals near the overflow edges, beside the small ones.
    const BIG: [i64; 5] = [i64::MAX, i64::MAX - 1, 1 << 32, 1 << 62, 3_037_000_499];

    enum Expr {
        Lit(Value),
        Neg(Box<Expr>),
        /// `+ - * / %`, both sides `i64` or both `u8`.
        Arith(&'static str, Box<Expr>, Box<Expr>),
        /// `== != < <= > >=`, both sides `i64` or both `u8`.
        Compare(&'static str, Box<Expr>, Box<Expr>),
        /// `&&` or `||`.
        Logic(&'static str, Box<Expr>, Box<Expr>),
        If(Box<Expr>, Box<Expr>, Box<Expr>),
    }

    /// `a op b` with Rust's checked arithmetic: truncating `/`, `%` with the dividend's sign.
    fn arith(op: &str, a: i64, b: i64) -> Option<i64> {
        match op {
            "+" => a.checked_add(b),
            "-" => a.checked_sub(b),
            "*" => a.checked_mul(b),
            "/" => a.checked_div(b),
            "%" => a.checked_rem(b),
            _ => unreachable!("not an arithmetic operator: {op}"),
        }
    }

    impl Expr {
        /// The Sisu source, every operator in parentheses so no precedence or chaining applies.
        fn render(&self) -> String {
            match self {
                Expr::Lit(Value::I64(n)) if *n < 0 => format!("(-{})", n.unsigned_abs()),
                Expr::Lit(Value::U8(n)) => format!("(u8({n}) ?? 0)"),
                Expr::Lit(v) => v.to_string(),
                Expr::Neg(e) => format!("(-{})", e.render()),
                Expr::Arith(op, l, r) | Expr::Compare(op, l, r) | Expr::Logic(op, l, r) => {
                    format!("({} {op} {})", l.render(), r.render())
                }
                Expr::If(c, a, b) => format!(
                    "(if {} {{ {} }} else {{ {} }})",
                    c.render(),
                    a.render(),
                    b.render()
                ),
            }
        }

        /// The value, or `None` where Sisu panics: overflow or division by zero. `&&`, `||`
        /// and `if` evaluate only what Sisu does.
        fn eval(&self) -> Option<Value> {
            Some(match self {
                Expr::Lit(v) => *v,
                Expr::Neg(e) => Value::I64(e.eval()?.int().checked_neg()?),
                Expr::Arith(op, l, r) => {
                    let (l, r) = (l.eval()?, r.eval()?);
                    let n = arith(op, l.int(), r.int())?;
                    match l {
                        Value::U8(_) => Value::U8(u8::try_from(n).ok()?),
                        _ => Value::I64(n),
                    }
                }
                Expr::Compare(op, l, r) => {
                    let (l, r) = (l.eval()?.int(), r.eval()?.int());
                    Value::Bool(match *op {
                        "==" => l == r,
                        "!=" => l != r,
                        "<" => l < r,
                        "<=" => l <= r,
                        ">" => l > r,
                        ">=" => l >= r,
                        _ => unreachable!("not a comparison: {op}"),
                    })
                }
                Expr::Logic(op, l, r) => {
                    let l = l.eval()?.truth();
                    // `&&` stops at false, `||` at true.
                    if l == (*op == "||") {
                        Value::Bool(l)
                    } else {
                        r.eval()?
                    }
                }
                Expr::If(c, a, b) => {
                    if c.eval()?.truth() {
                        a.eval()?
                    } else {
                        b.eval()?
                    }
                }
            })
        }

        /// An expression of type `ty`, at most `depth` operators deep.
        fn generate(rng: &mut XorShift, ty: Ty, depth: u32) -> Expr {
            let boxed = |rng: &mut XorShift, ty| Box::new(Expr::generate(rng, ty, depth - 1));
            // A leaf at depth 0, and one time in four above it.
            let choice = if depth == 0 || rng.next().is_multiple_of(4) {
                0
            } else {
                1 + rng.next() % 3
            };
            match (ty, choice) {
                (Ty::I64, 0) => Expr::Lit(Value::I64(if rng.next().is_multiple_of(4) {
                    let big = rng.pick(&BIG);
                    if rng.next().is_multiple_of(2) {
                        big
                    } else {
                        -big
                    }
                } else {
                    i64::try_from(rng.next() % 41).expect("below 41 fits i64") - 20
                })),
                (Ty::U8, 0) => Expr::Lit(Value::U8(rng.pick(&[0, 1, 2, 3, 7, 16, 100, 200, 255]))),
                (Ty::Bool, 0) => Expr::Lit(Value::Bool(rng.next().is_multiple_of(2))),
                (Ty::I64, 1) => Expr::Neg(boxed(rng, Ty::I64)),
                (Ty::I64 | Ty::U8, 1 | 2) => {
                    Expr::Arith(rng.pick(&ARITH), boxed(rng, ty), boxed(rng, ty))
                }
                (Ty::Bool, 1) => {
                    let operands = rng.pick(&[Ty::I64, Ty::U8]);
                    Expr::Compare(
                        rng.pick(&COMPARE),
                        boxed(rng, operands),
                        boxed(rng, operands),
                    )
                }
                (Ty::Bool, 2) => {
                    Expr::Logic(rng.pick(&["&&", "||"]), boxed(rng, ty), boxed(rng, ty))
                }
                _ => Expr::If(boxed(rng, Ty::Bool), boxed(rng, ty), boxed(rng, ty)),
            }
        }
    }

    /// Fifty expressions per seed, each regenerated until Rust can evaluate it, printed by one
    /// program, which both builds must run to exactly the lines Rust computes.
    #[test]
    fn generated_arithmetic_matches_rust() {
        for seed in 1..=4 {
            let mut rng = XorShift(seed);
            let exprs: Vec<(String, Value)> = (0..50)
                .map(|i| {
                    let ty = [Ty::I64, Ty::U8, Ty::Bool][i % 3];
                    loop {
                        let e = Expr::generate(&mut rng, ty, 4);
                        if let Some(value) = e.eval() {
                            break (e.render(), value);
                        }
                    }
                })
                .collect();
            let prints: Vec<String> = exprs
                .iter()
                .map(|(src, _)| format!("    print({src})"))
                .collect();
            let source = format!("fn main() {{\n{}\n}}\n", prints.join("\n"));
            for o2 in [false, true] {
                let exe = compile_tmp(&format!("generated_{seed}"), &source, o2);
                let out = Command::new(&exe).output().expect("the program starts");
                let stdout = String::from_utf8_lossy(&out.stdout);
                let lines: Vec<&str> = stdout.lines().collect();
                let first_wrong = exprs
                    .iter()
                    .enumerate()
                    .find(|(i, (_, value))| lines.get(*i) != Some(&value.to_string().as_str()));
                if let Some((i, (src, value))) = first_wrong {
                    panic!(
                        "seed {seed}, {} build, expression {i}: `{src}` printed {:?}, Rust \
                         says {value}; exit {:?}, stderr:\n{}",
                        if o2 { "-O2" } else { "O0" },
                        lines.get(i),
                        out.status,
                        String::from_utf8_lossy(&out.stderr)
                    );
                }
                assert_eq!(lines.len(), exprs.len(), "seed {seed}: extra output");
                assert_eq!(out.status.code(), Some(0), "seed {seed}: {:?}", out.status);
            }
        }
    }
}
