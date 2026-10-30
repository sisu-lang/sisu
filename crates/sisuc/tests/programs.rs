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

mod scratch_path;
mod support;

use scratch_path::{empty_dir, only_cc};
use support::fixture;
use support::selfhost;

const PROGRAMS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/programs");
const RUN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/run");
const MIN: &str = "    let min = -9_223_372_036_854_775_807 - 1";
const MAX: &str = "    let big = 9_223_372_036_854_775_807";

/// Compiles `<dir>/<name>.sisu`, run from `dir`, into `<CARGO_TARGET_TMPDIR>/<name>`, or
/// `<name>-O2` with `o2`, passing `flags` to `sisu` too.
fn compile_to_tmp(dir: &Path, name: &str, o2: bool, flags: &[&str]) -> PathBuf {
    let exe = Path::new(env!("CARGO_TARGET_TMPDIR")).join(if o2 {
        format!("{name}-O2")
    } else {
        name.to_string()
    });
    let flags: Vec<&str> = o2
        .then_some("-O2")
        .into_iter()
        .chain(flags.iter().copied())
        .collect();
    support::compile(dir, name, &exe, &flags);
    exe
}

/// Checks that `out` is a panic: exit 101, `stdout`, and exactly the panic line on stderr.
fn assert_panics(out: &Output, stdout: &str, stderr: &str) {
    assert_eq!(out.status.code(), Some(101), "the program should panic");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        stdout,
        "stdout should match"
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        stderr,
        "stderr should match"
    );
}

/// The programs the small-stack tests run. They are not `tests/run` cases: many objects
/// under Valgrind is too slow, and `deepchain` overflows on purpose.
const SMALL_STACK: [&str; 23] = [
    "longlist",
    "rightskew",
    "deepchain",
    "deepchain2",
    "dropmutual",
    "dropoptional",
    "dropmutualoptional",
    "enumlist",
    "enumfold",
    "enummutual",
    "enumeq",
    "deeppair",
    "dropleft",
    "eqleft",
    "eqorder",
    "eqtrail",
    "tailfresh",
    "tailpass",
    "showright",
    "showleft",
    "showmutual",
    "showinline",
    "showoptionallink",
];

/// GIVEN the `tests/programs` directory
/// WHEN its files are listed
/// THEN they are exactly the `.sisu` files of the `SMALL_STACK` programs
#[test]
fn programs_directory_should_hold_only_the_small_stack_programs() {
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

/// Builds `<name>.sisu` twice, with its first line `fn iterations() -> i64 { 1000 }` replaced by
/// `fn iterations() -> i64 { (read_line() ?? "").len() + k }`, for `k` = 1000 and `k` = 0, at `-O2`
/// if `o2`. Returns both builds' stdout and the difference in allocations. The `k` = 0 build is
/// the baseline: the runtime's own allocations cancel.
fn extra_allocs(name: &str, o2: bool) -> (String, String, i64) {
    const FIRST_LINE: &str = "fn iterations() -> i64 { 1000 }\n";
    let source =
        fs::read_to_string(Path::new(RUN).join(format!("{name}.sisu"))).expect("reads the program");
    let rest = source
        .strip_prefix(FIRST_LINE)
        .unwrap_or_else(|| panic!("{name}.sisu should start with `{}`", FIRST_LINE.trim_end()));
    // Standard input is empty, so `iterations()` is `k`, but LLVM cannot know it. A constant `0`
    // makes the loop dead, and at `-O2` LLVM deletes an allocation that nothing reads, which the
    // other build still makes.
    let build =
        |k: &str| format!("fn iterations() -> i64 {{ (read_line() ?? \"\").len() + {k} }}\n{rest}");

    // Own file names, so these builds never share an executable with the test of `<name>.out`.
    let (stdout, allocs) = heap_allocs(&compile_tmp_for_valgrind(
        &format!("{name}_k1000"),
        &build("1000"),
        o2,
    ));
    let (base_stdout, base_allocs) = heap_allocs(&compile_tmp_for_valgrind(
        &format!("{name}_k0"),
        &build("0"),
        o2,
    ));
    let extra = i64::try_from(allocs).expect("a count fits i64")
        - i64::try_from(base_allocs).expect("a count fits i64");
    (stdout, base_stdout, extra)
}

/// GIVEN a Valgrind summary line `total heap usage: 1,004 allocs`
/// WHEN `allocs_in` reads it
/// THEN it returns 1004
#[test]
fn allocs_in_should_read_a_count_with_thousands_separators() {
    let stderr = "==1== total heap usage: 1,004 allocs, 1,004 frees, 9 bytes allocated";
    assert_eq!(allocs_in(stderr), 1004);
}

/// GIVEN Valgrind output that has no `total heap usage` line
/// WHEN `allocs_in` reads it
/// THEN it panics and names the missing line
#[test]
#[should_panic(expected = "no `total heap usage` line")]
fn allocs_in_should_panic_when_the_summary_line_is_missing() {
    allocs_in("==1== All heap blocks were freed -- no leaks are possible");
}

/// `first`, then the numbers 0 up to `count`, each on a line.
fn numbered_lines(first: &str, count: u32) -> String {
    use std::fmt::Write as _;
    (0..count).fold(format!("{first}\n"), |mut out, n| {
        writeln!(out, "{n}").expect("writing to a `String` should succeed");
        out
    })
}

/// One test per row, `allocation_counts::<program>`: (program, stdout at k = 1000, stdout at
/// k = 0, extra allocations), checked against `extra_allocs` at both optimization levels.
macro_rules! allocation_counts {
    ($(($name:ident, $stdout:expr, $base_stdout:expr, $extra:expr),)*) => {
        mod allocation_counts {
            $(
                #[test]
                #[cfg_attr(
                    not(target_os = "linux"),
                    ignore = "reads Valgrind's output, and Valgrind runs only on Linux"
                )]
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
    // `xs` holds its array once, so each `push` is in place: the block of the literal, then a
    // reallocation at each doubling to 1024. Valgrind counts each `realloc` as an allocation.
    (push_local, "1001\n", "1\n", 10),
    // `saved` is live across each push, so each push copies once.
    (push_shared, "1001\n1000\n", "1\n1\n", 1000),
    // A field or element push is in place too.
    (push_field, "1001\n", "1\n", 10),
    (push_element, "1001\n", "1\n", 10),
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
    // The same through a box with count one: no array copy. Building into the old box
    // allocates nothing.
    (enum_alloc_handover_box, "1000\n", "0\n", 0),
    // Through a shared box: one new box and one array copy per step.
    (enum_alloc_shared_box, "1000\n499500\n", "0\n0\n", 2000),
    // Each step takes `e`'s two boxes apart for the new `Add`. Building into the old boxes
    // allocates nothing.
    (enum_alloc_rewrite, "12\n", "12\n", 0),
    // The same for an `Option`-shaped enum, whose value is the pointer to its box.
    (enum_alloc_chain_rewrite, "3\n", "3\n", 0),
    // `==` on right-deep values needs no buffer of pending pairs: only the 4 boxes per level.
    (enum_alloc_eq_right, "true\n", "true\n", 4000),
    // `print` of an enum writes into the output buffer with no text buffer: the pending list of
    // a right-leaning value stays in its first slot in the frame, so nothing is allocated. The
    // 4 boxes of the one value are built in both runs.
    (show_print, "start\n".to_string() + &"Add(Num(1), Add(Num(2), Num(3)))\n".repeat(1000), "start\n", 0),
    // A left-leaning value has two entries waiting at once, and the second spills the pending list
    // to the heap: one block for each `print`.
    (show_print_left, "start\n".to_string() + &"Add(Add(Num(1), Num(2)), Num(3))\n".repeat(1000), "start\n", 1000),
    // `e.to_string()` of an enum is one block, the result string, for each call and none for a
    // node, whatever the size of the tree.
    (show_string_small, "Num(1)\n", "-\n", 1000),
    (show_string_tree, "Add(Num(1), Mul(Num(2), Num(3)))\n", "-\n", 1000),
    // An interpolation with a compound part is one block too.
    (show_interp, "Add(Num(1), Mul(Num(2), Num(3)))!\n", "-\n", 1000),
    // `print` of an `i64` allocates nothing.
    (print_int, super::numbered_lines("start", 1000), "start\n", 0),
}

/// Compiles `tests/programs/<name>.sisu` without `-O2` and runs it under a 256 KiB stack.
fn run_in_small_stack(name: &str) -> Output {
    run_in_small_stack_at(name, false)
}

/// `run_in_small_stack` with `-O2` if `o2`.
fn run_in_small_stack_at(name: &str, o2: bool) -> Output {
    let exe = compile_to_tmp(Path::new(PROGRAMS), name, o2, &[]);
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
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        expected,
        "stdout should match"
    );
}

/// GIVEN the `longlist` program, which builds a list of 100 000 nodes
/// WHEN it runs under a 256 KiB stack
/// THEN it exits 0 and prints 100000
#[test]
fn longlist_should_free_a_long_list_in_a_small_stack() {
    assert_small_stack_prints("longlist", "100000\n");
}

/// GIVEN the `rightskew` program, which builds a right-skewed tree of 100 000 nodes
/// WHEN it runs under a 256 KiB stack
/// THEN it exits 0 and prints 100000
#[test]
fn rightskew_should_free_a_long_tree_in_a_small_stack() {
    assert_small_stack_prints("rightskew", "100000\n");
}

/// `assert_small_stack_prints` at both levels: `-O2` must not need more stack either.
fn assert_small_stack_prints_at_both(name: &str, expected: &str) {
    for o2 in [false, true] {
        let out = run_in_small_stack_at(name, o2);
        assert_eq!(out.status.code(), Some(0), "-O2 {o2}: {:?}", out.status);
        assert_eq!(String::from_utf8_lossy(&out.stdout), expected, "-O2 {o2}");
    }
}

/// GIVEN the `enumlist` program, which builds a right-deep enum list of 100 000 cells
/// WHEN it runs under a 256 KiB stack, with and without `-O2`
/// THEN it exits 0 and prints 100000
///
/// A right-deep enum list of 100 000 cells frees its boxes from the pending list.
#[test]
fn enum_list_should_free_in_a_small_stack() {
    assert_small_stack_prints_at_both("enumlist", "100000\n");
}

/// GIVEN the `enumfold` program, which builds a left-deep fold of 100 000 `Add`s
/// WHEN it runs under a 256 KiB stack, with and without `-O2`
/// THEN it exits 0 and prints 100000
///
/// A left-deep fold of 100 000 `Add`s frees its boxes from the pending list.
#[test]
fn enum_fold_should_free_in_a_small_stack() {
    assert_small_stack_prints_at_both("enumfold", "100000\n");
}

/// GIVEN the `enummutual` program, which holds two chains through two enums that hold each other
/// WHEN it runs under a 256 KiB stack, with and without `-O2`
/// THEN it exits 0 and prints 100000 and `true`
///
/// Two chains through two enums that hold each other compare with `==` and free in one frame.
#[test]
fn mutual_enums_should_compare_and_free_in_a_small_stack() {
    assert_small_stack_prints_at_both("enummutual", "100000\ntrue\n");
}

/// GIVEN the `enumeq` program, which compares two pairs of enum lists of 100 000 cells
/// WHEN it runs under a 256 KiB stack, with and without `-O2`
/// THEN it exits 0 and prints `true` and `false`
///
/// `==` on two pairs of 100 000-cell enum lists loops on the tail.
#[test]
fn equal_enum_lists_should_compare_in_a_small_stack() {
    assert_small_stack_prints_at_both("enumeq", "true\nfalse\n");
}

/// GIVEN the `deeppair` program, which compares and drops two pairs of 100 000-deep `Expr`s,
/// deep through the inline field of a generic enum
/// WHEN it runs under a 256 KiB stack, with and without `-O2`
/// THEN it exits 0 and prints `true` and `false`
///
/// `==` and the drop walk the inline field of a generic enum in place.
#[test]
fn deep_inline_links_should_compare_and_free_in_a_small_stack() {
    assert_small_stack_prints_at_both("deeppair", "true\nfalse\n");
}

/// GIVEN the `deepchain2` program, which compares and drops two pairs of 100 000-deep `Chain`s
/// (a `Chain` is an `Option`-shaped enum that holds itself)
/// WHEN it runs under a 256 KiB stack, with and without `-O2`
/// THEN it exits 0 and prints `true` and `false`
///
/// `==` and the drop of an enum that is the pointer to its box loop on the box in one frame.
#[test]
fn option_shaped_chains_should_compare_and_free_in_a_small_stack() {
    assert_small_stack_prints_at_both("deepchain2", "true\nfalse\n");
}

/// GIVEN the `dropleft` program, which builds a class tree 100 000 deep through `l`, not the
/// chain field, and one that alternates `l` and `r`
/// WHEN it runs under a 256 KiB stack, with and without `-O2`
/// THEN it exits 0 and prints `done`
#[test]
fn class_drop_should_free_trees_deep_through_any_own_field_in_a_small_stack() {
    assert_small_stack_prints_at_both("dropleft", "done\n");
}

/// GIVEN the `eqleft` program, which compares three pairs of left-deep trees of 100 000 nodes
/// WHEN it runs under a 256 KiB stack, with and without `-O2`
/// THEN it exits 0 and prints `true`, `false` and `false`
///
/// `==` keeps the first fields it still has to compare on the heap, not on the stack.
#[test]
fn left_deep_enums_should_compare_in_a_small_stack() {
    assert_small_stack_prints_at_both("eqleft", "true\nfalse\nfalse\n");
}

/// GIVEN the `eqorder` program, whose two pairs of enum values differ in their first fields and
/// hold 20 000-deep class chains in a later field, the second of two or three
/// WHEN it runs under a 256 KiB stack, with and without `-O2`
/// THEN it exits 0 and prints `false` twice
///
/// `==` compares the fields in order and finds the first mismatch before it compares the chains,
/// whose `==` would overflow.
#[test]
fn enum_eq_should_stop_at_the_first_mismatch_in_a_small_stack() {
    assert_small_stack_prints_at_both("eqorder", "false\nfalse\n");
}

/// How deep the values of the `show` programs are.
const SHOW_DEPTH: usize = 1_000_000;

/// Runs `<name>.sisu` under the small stack at both levels and checks that it prints `text`, the
/// value's text, and then `done`. Each text is 9 to 20 MB, and the test builds it from the depth.
fn assert_deep_value_prints(name: &str, text: &str) {
    assert_small_stack_prints_at_both(name, &format!("{text}\ndone\n"));
}

/// GIVEN the `showright` program, which builds a right-deep list of 1,000,000 cells
/// WHEN it runs `print(e)` and then `print("done")` under a 256 KiB stack, with and without `-O2`
/// THEN it exits 0 and prints `Cons(7, ` 1,000,000 times, `Nil`, 1,000,000 `)` and `done`
///
/// The `show` helper loops on the last link and counts the `)` it owes, not in a frame each. One
/// test for each shape of `printing_deep_values_should_fit_a_small_stack`, so that they run
/// at once.
#[test]
fn printing_a_right_deep_list_should_fit_a_small_stack() {
    let text = "Cons(7, ".repeat(SHOW_DEPTH) + "Nil" + &")".repeat(SHOW_DEPTH);
    assert_deep_value_prints("showright", &text);
}

/// GIVEN the `showleft` program, which builds a left-deep tree of 1,000,000 nodes
/// WHEN it runs `print(e)` and then `print("done")` under a 256 KiB stack, with and without `-O2`
/// THEN it exits 0 and prints `Node(` 1,000,000 times, `Leaf`, 1,000,000 `, Leaf)` and `done`
///
/// The helper keeps the places of the first fields on the heap.
#[test]
fn printing_a_left_deep_tree_should_fit_a_small_stack() {
    let text = "Node(".repeat(SHOW_DEPTH) + "Leaf" + &", Leaf)".repeat(SHOW_DEPTH);
    assert_deep_value_prints("showleft", &text);
}

/// GIVEN the `showmutual` program, which builds a chain of 1,000,000 levels of each of two enums
/// that hold each other
/// WHEN it runs `print(e)` and then `print("done")` under a 256 KiB stack, with and without `-O2`
/// THEN it exits 0 and prints `ToA(ToB(7, ` 1,000,000 times, `End`, 1,000,000 `))` and `done`
///
/// The helper steps from one enum to the other in one frame.
#[test]
fn printing_two_enums_that_hold_each_other_should_fit_a_small_stack() {
    let text = "ToA(ToB(7, ".repeat(SHOW_DEPTH) + "End" + &"))".repeat(SHOW_DEPTH);
    assert_deep_value_prints("showmutual", &text);
}

/// GIVEN the `showinline` program, which builds a chain of 1,000,000 `Pair`s, deep through the
/// inline second field of `Both`
/// WHEN it runs `print(e)` and then `print("done")` under a 256 KiB stack, with and without `-O2`
/// THEN it exits 0 and prints `Pair(Both(Lit(7), ` 1,000,000 times, `Lit(7)`, 1,000,000 `))` and
/// `done`
///
/// The helper walks an inline link in place.
#[test]
fn printing_an_inline_link_should_fit_a_small_stack() {
    let text = "Pair(Both(Lit(7), ".repeat(SHOW_DEPTH) + "Lit(7)" + &"))".repeat(SHOW_DEPTH);
    assert_deep_value_prints("showinline", &text);
}

/// GIVEN the `showoptionallink` program, which builds a chain of 1,000,000 `Pair`s, deep through
/// an inline field of type `Expr??`
/// WHEN it prints the chain, compares it with an equal chain and drops it under a 256 KiB stack,
/// with and without `-O2`
/// THEN it exits 0 and prints `Pair(Both(` 1,000,000 times, `Lit(7)`, 1,000,000 `))`, `true` and
/// `done`
///
/// A link under more than one `?` is walked in place, as a link under one is: printing, `==` and
/// the drop each loop on it.
#[test]
fn printing_an_inline_link_under_two_optionals_should_fit_a_small_stack() {
    let text = "Pair(Both(".repeat(SHOW_DEPTH) + "Lit(7)" + &"))".repeat(SHOW_DEPTH);
    assert_small_stack_prints_at_both("showoptionallink", &format!("{text}\ntrue\ndone\n"));
}

/// GIVEN the `dropmutual` program, which builds chains of 100 000 pairs of two classes that
/// hold each other and of 30 000 triples of three classes that hold each other
/// WHEN it runs under a 256 KiB stack, with and without `-O2`
/// THEN it exits 0 and prints 100000 and 30000
///
/// The drop frees the objects of the classes in a cycle from pending lists, not by recursion.
#[test]
fn class_drop_should_free_chains_of_classes_that_hold_each_other_in_a_small_stack() {
    assert_small_stack_prints_at_both("dropmutual", "100000\n30000\n");
}

/// GIVEN the `dropoptional` program, which builds a list of 100 000 nodes through a field of
/// type `Node??`, the last link `Some(None)`
/// WHEN it runs under a 256 KiB stack, with and without `-O2`
/// THEN it exits 0 and prints 100000
///
/// The drop follows a field with more than one `?` in a loop, as it follows `Node?`.
#[test]
fn nested_optional_link_should_free_a_long_list_in_a_small_stack() {
    assert_small_stack_prints_at_both("dropoptional", "100000\n");
}

/// GIVEN the `dropmutualoptional` program, which builds a chain of 100 000 pairs of two
/// classes that hold each other through fields of type `A??` and `B??`
/// WHEN it runs under a 256 KiB stack, with and without `-O2`
/// THEN it exits 0 and prints 100000
///
/// The drop of a cycle of classes frees the objects from pending lists, whatever the number of
/// `?` of the fields that link them.
#[test]
fn nested_optional_links_between_classes_should_free_in_a_small_stack() {
    assert_small_stack_prints_at_both("dropmutualoptional", "100000\n");
}

/// GIVEN the `eqtrail` program, which compares two pairs of left-deep values of 100 000 nodes
/// whose links are followed by an `i64` field
/// WHEN it runs under a 256 KiB stack, with and without `-O2`
/// THEN it exits 0 and prints `true` and `false`
///
/// `==` keeps a link that has a field after it on the heap, not on the stack.
#[test]
fn enums_with_a_field_after_the_link_should_compare_in_a_small_stack() {
    assert_small_stack_prints_at_both("eqtrail", "true\nfalse\n");
}

/// GIVEN the `deepchain` program, which holds 500 000 objects each in an array of the one before
/// WHEN it runs under a 256 KiB stack
/// THEN it dies by signal 11 and prints nothing
///
/// The control for `longlist_should_free_a_long_list_in_a_small_stack`: a drop that
/// recurses once per object, as the drop of an array does, overflows the same stack, so the
/// limit is in force.
#[test]
fn deepchain_should_overflow_the_small_stack() {
    let out = run_in_small_stack("deepchain");
    assert_eq!(out.status.signal(), Some(11), "{:?}", out.status);
    // It dies in the drop, after `print`, but stdout is a pipe and so block-buffered: the
    // runtime flushes only on a normal exit or a Sisu panic, so a signal loses the line, as
    // it does in a C program.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
}

/// Runs `<name>.sisu` at `-O2` under the small stack and checks that it exits 0 and prints
/// `expected`.
fn assert_small_stack_prints_at_o2(name: &str, expected: &str) {
    let out = run_in_small_stack_at(name, true);
    assert_eq!(out.status.code(), Some(0), "{:?}", out.status);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        expected,
        "stdout should match"
    );
}

/// GIVEN the `tailfresh` program, with 100 000 tail calls that each pass a fresh string
/// WHEN it is built with `-O2` and runs under a 256 KiB stack
/// THEN it exits 0 and prints 100000
///
/// The tail rule: `walk` owns the fresh string it hands itself, so the caller releases nothing
/// after the call, and `-O2` makes 100 000 tail calls a loop.
#[test]
fn tail_call_with_a_fresh_string_should_run_in_a_small_stack() {
    assert_small_stack_prints_at_o2("tailfresh", "100000\n");
}

/// GIVEN the `tailpass` program, whose tail call passes a borrowed string on
/// WHEN it is built with `-O2` and runs under a 256 KiB stack
/// THEN it exits 0 and prints 2
///
/// `walk` passes its borrowed string on as it is, so its tail call stays a tail call.
#[test]
fn tail_call_with_a_borrowed_string_should_run_in_a_small_stack() {
    assert_small_stack_prints_at_o2("tailpass", "2\n");
}

/// GIVEN the `tailpass` program with 100 000 nested calls
/// WHEN it is built without `-O2` and runs under a 256 KiB stack
/// THEN it dies by signal 11 and prints nothing
///
/// The control for the two tail-call tests: without `-O2`, no call is a tail call, and 100 000
/// frames overflow the same stack.
#[test]
fn tail_calls_should_overflow_the_small_stack_without_optimization() {
    let out = run_in_small_stack("tailpass");
    assert_eq!(out.status.signal(), Some(11), "{:?}", out.status);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
}

/// Writes `source` to `<CARGO_TARGET_TMPDIR>/<name>.sisu` and compiles it from there, at
/// `-O2` if `o2`.
fn compile_tmp(name: &str, source: &str, o2: bool) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join(format!("{name}.sisu")), source).expect("writes");
    compile_to_tmp(dir, name, o2, &[])
}

/// `compile_tmp` with `support::VALGRIND_FLAGS`, for a build that `support::Invocation::run`
/// runs under Valgrind.
fn compile_tmp_for_valgrind(name: &str, source: &str, o2: bool) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join(format!("{name}.sisu")), source).expect("writes");
    compile_to_tmp(dir, name, o2, support::VALGRIND_FLAGS)
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
    // 24 + 9_223_372_036_854_775_783 bytes is exactly `i64::MAX`, above the largest
    // size that rounds up to the alignment of 8 within it.
    length_too_large => fill_case("false", "9_223_372_036_854_775_783", 22),
    // The smallest length past `largest_array_should_reach_the_allocator_and_abort`'s: 24 +
    // 9_223_372_036_854_775_777 is `i64::MAX - 6`, which rounds up to 2^63.
    one_too_large => fill_case("false", "9_223_372_036_854_775_777", 22),
    // 2^62 * 8 bytes overflows the multiply.
    length_overflows => fill_case("0", "4_611_686_018_427_387_904", 18),
    // (2^61 - 1) * 8 bytes is 2^64 - 8, which fits; adding the 24-byte header wraps.
    header_overflows => fill_case("0", "2_305_843_009_213_693_951", 18),
}

/// A `panic_cases` row for `grid[0][9] = true`: the inner index is checked at its own level,
/// after the outer one.
fn write_bounds_case() -> (String, String) {
    (
        "fn main() {\n    let mut grid = [[false, false, false]]\n    grid[0][9] = true\n    print(grid[0][0])\n}\n"
            .to_string(),
        "3:13: index 9 out of bounds for length 3".to_string(),
    )
}

/// A `panic_cases` row for `names[i] += say()` with `i` past the end: the append reads the
/// element's slot, bounds-checked, before `say()` runs. `say` would print, so the empty stdout
/// `panic_cases` expects shows the panic comes first.
fn append_bounds_case() -> (String, String) {
    (
        "fn say() -> string {\n    print(\"said\")\n    \"x\"\n}\nfn main() {\n    let mut names = [\"a\", \"b\"]\n    let i = 3\n    names[i] += say()\n    print(names[0])\n}\n"
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

/// GIVEN a program that makes the largest array that the size checks allow
/// WHEN it is built and run, with and without `-O2`
/// THEN it dies by signal 6 and its first stderr line says that the allocation of
/// 9223372036854775800 bytes failed
///
/// The largest block the size checks allow, `i64::MAX` rounded down to the alignment of 8,
/// passes them and reaches the allocator, which cannot provide it: `sisu_alloc` calls
/// `handle_alloc_error` rather than aborting on an invalid layout, which prints nothing. The
/// program loops over the elements: LLVM cannot work out the loads from the fill, so the allocation
/// stays at `-O2`, while a single `xs[0]` read lets it vanish.
#[test]
fn largest_array_should_reach_the_allocator_and_abort() {
    // 24 + 9_223_372_036_854_775_776 bytes is 9_223_372_036_854_775_800.
    let source = "fn main() {\n    let xs = [false; 9_223_372_036_854_775_776]\n    let mut k = 0\n    for x in xs { if x { k += 1 } }\n    print(k)\n}\n";
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

/// GIVEN a program built by default and a program built with `--allocator system`
/// WHEN each runs with `MIMALLOC_VERBOSE=1`
/// THEN both exit 0 and print 1
/// AND only the default build writes `mimalloc:` lines to stderr
#[test]
fn allocator_flag_should_pick_the_runtime_archive() {
    let source = "fn main() {\n    print(1)\n}\n";
    let default = compile_tmp("allocator_default", source, false);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let system = dir.join("allocator_system");
    support::compile(
        dir,
        "allocator_default",
        &system,
        &["--allocator", "system"],
    );
    for (exe, mimalloc) in [(default, true), (system, false)] {
        let out = Command::new(&exe)
            .env("MIMALLOC_VERBOSE", "1")
            .output()
            .expect("the program starts");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{}: {stderr}", exe.display());
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "1\n",
            "{}",
            exe.display()
        );
        assert_eq!(
            stderr.contains("mimalloc:"),
            mimalloc,
            "{}: {stderr}",
            exe.display()
        );
    }
}

/// What the runtime prints when a `print` cannot write to standard output.
const STDOUT_WRITE_PANIC: &str =
    "sisu: panic: standard output could not be written: No space left on device (os error 28)\n";

/// GIVEN a program that prints an `i64` or a `bool`
/// AND standard output is `/dev/full`
/// WHEN it is built and run, with and without `-O2`
/// THEN it exits 101 and writes only the Sisu panic line about standard output to stderr
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "writes to /dev/full, which only Linux has"
)]
fn print_should_panic_with_exit_101_when_stdout_is_full() {
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

/// GIVEN a program that panics on an index out of bounds
/// AND standard error is a pipe with no reader
/// WHEN it is built and run, with and without `-O2`
/// THEN it exits 101
///
/// A Sisu panic whose stderr has no reader still exits 101: the runtime ignores SIGPIPE
/// before it writes the panic line. This runs a compiled program, because the libtest child
/// of a runtime unit test starts with SIGPIPE ignored already.
#[test]
fn panic_should_exit_101_when_stderr_is_a_closed_pipe() {
    let source = "fn main() {\n    let xs = [1, 2]\n    let i = 2\n    print(xs[i])\n}\n";
    for o2 in [false, true] {
        let status = Command::new(compile_tmp("panic_closed_stderr", source, o2))
            .stderr(broken_pipe())
            .status()
            .expect("the program starts");
        assert_eq!(status.code(), Some(101), "{status:?}");
    }
}

/// GIVEN a program that prints 1
/// AND standard output is a pipe with no reader
/// WHEN it is built and run, with and without `-O2`
/// THEN it dies by signal 13
///
/// Standard output with no reader keeps the default SIGPIPE action: the program dies by
/// signal 13, as a C program does.
#[test]
fn print_should_die_by_sigpipe_when_stdout_is_a_closed_pipe() {
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

/// GIVEN a program that prints 1 and `true`, then reads an array past its end
/// WHEN it runs with standard output and standard error on one pipe, with and without `-O2`
/// THEN it exits 101
/// AND the pipe holds `1`, `true` and then the panic line, in that order
///
/// Buffered `print` lines come out before the panic line, in order.
#[test]
fn panic_should_flush_printed_lines_before_the_panic_line() {
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

/// GIVEN a program that prints 1 and `true`, then calls `panic("x")`
/// WHEN it runs with standard output and standard error on one pipe, with and without `-O2`
/// THEN it exits 101
/// AND the pipe holds `1`, `true` and then the panic line, in that order
///
/// A `panic` call flushes the buffered `print` lines before its panic line, as a run-time
/// panic does.
#[test]
fn panic_call_should_flush_printed_lines_before_the_panic_line() {
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

/// GIVEN a program that prints three values and ends normally
/// AND standard output is `/dev/full`
/// WHEN it is built and run, with and without `-O2`
/// THEN it exits 101 and writes only one panic line to stderr
///
/// Output is buffered: only the final flush at the end of `main` fails, and it ends the
/// program with the one panic line, not a second error when the process exits.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "writes to /dev/full, which only Linux has"
)]
fn final_flush_should_panic_once_when_stdout_is_full() {
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

/// GIVEN a program whose only `print` is of a value whose written `to_string` gives ""
/// AND standard output is `/dev/full`
/// WHEN it is built and run, with and without `-O2`
/// THEN it exits 101 and writes only one panic line to stderr
///
/// The text has no piece, so the line end alone opens standard output. The newline must reach
/// the final flush, which fails.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "writes to /dev/full, which only Linux has"
)]
fn printing_an_empty_text_should_panic_when_stdout_is_full() {
    let source = "class Quiet {}\nextend Quiet: Display {\n    fn to_string(self) -> string { \"\" }\n}\nfn main() {\n    print(Quiet())\n}\n";
    for o2 in [false, true] {
        let full = File::options()
            .write(true)
            .open("/dev/full")
            .expect("/dev/full exists on Linux");
        let out = Command::new(compile_tmp("print_empty_full", source, o2))
            .stdout(full)
            .output()
            .expect("the program starts");
        assert_eq!(out.status.code(), Some(101), "{:?}", out.status);
        assert_eq!(String::from_utf8_lossy(&out.stderr), STDOUT_WRITE_PANIC);
    }
}

/// GIVEN a program that prints 1
/// AND standard output is closed
/// WHEN it is built and run, with and without `-O2`
/// THEN it exits 0 and writes nothing to stderr
///
/// A closed standard output drops what `print` writes and the program exits 0, as with
/// `println!`.
#[test]
fn print_should_be_dropped_when_stdout_is_closed() {
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

/// GIVEN a program that prints 1, calls `eprint(2)` and prints 3
/// AND standard error is closed
/// WHEN it is built and run, with and without `-O2`
/// THEN it exits 0 and prints 1 and 3 to stdout
///
/// A closed standard error drops what `eprint` writes and the program exits 0.
#[test]
fn eprint_should_be_dropped_when_stderr_is_closed() {
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

/// GIVEN a program that prints 1 and then calls `panic("x")`
/// AND standard error is closed
/// WHEN it is built and run, with and without `-O2`
/// THEN it exits 101 and prints 1 to stdout
///
/// A `panic` with a closed standard error still flushes standard output and exits 101.
#[test]
fn panic_should_flush_stdout_and_exit_101_when_stderr_is_closed() {
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

/// GIVEN a program that calls `eprint(1)`
/// AND standard error is a pipe with no reader
/// WHEN it is built and run, with and without `-O2`
/// THEN it dies by signal 13
///
/// Standard error with no reader keeps the default SIGPIPE action: the program dies by
/// signal 13, as it does for `print` on standard output.
#[test]
fn eprint_should_die_by_sigpipe_when_stderr_is_a_closed_pipe() {
    let source = "fn main() {\n    eprint(1)\n}\n";
    for o2 in [false, true] {
        let status = Command::new(compile_tmp("eprint_closed_pipe", source, o2))
            .stderr(broken_pipe())
            .status()
            .expect("the program starts");
        assert_eq!(status.signal(), Some(13), "{status:?}");
    }
}

/// GIVEN a program that calls `eprint(1)` and prints 2
/// AND standard error is `/dev/full`
/// WHEN it is built and run, with and without `-O2`
/// THEN it exits 0 and prints 2 to stdout
///
/// A failed write to standard error (`ENOSPC`) is ignored: the program goes on and exits 0.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "writes to /dev/full, which only Linux has"
)]
fn eprint_should_be_ignored_when_stderr_is_full() {
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

/// GIVEN a program that prints 1 and then calls `eprint(2)`
/// WHEN it runs with standard output and standard error on one pipe, with and without `-O2`
/// THEN it exits 0 and the pipe holds `2` before `1`
///
/// `eprint` does not flush standard output first: on one pipe the `eprint` line comes before an
/// earlier `print` line, which waits for the exit.
#[test]
fn eprint_should_not_wait_for_earlier_print_lines() {
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

/// GIVEN a line of input and an argument that each hold an invalid UTF-8 byte
/// WHEN programs that echo them are built and run, with and without `-O2`
/// THEN both exit 0 and print the three bytes of `U+FFFD` for the invalid byte
///
/// An invalid byte in a line of input, or in an argument, comes back as the three bytes of
/// `U+FFFD`, which a `.out` file, read through `from_utf8_lossy`, cannot tell from the raw byte.
#[test]
fn invalid_utf8_should_become_the_replacement_character_in_input_and_arguments() {
    for o2 in [false, true] {
        let exe = compile_tmp("echo_repair", ECHO, o2);
        let out = support::Invocation::new(b"a\xFFb\n".to_vec(), Vec::new()).run(&exe, false);
        assert_eq!(out.status.code(), Some(0), "{:?}", out.status);
        assert_eq!(out.stdout, b"a\xEF\xBF\xBDb\n");
        let exe = compile_tmp_for_valgrind("echo_args_repair", ECHO_ARGS, o2);
        let arg = OsStr::from_bytes(b"x\xFFy").to_os_string();
        let out = support::Invocation::new(Vec::new(), vec![arg]).run(&exe, true);
        assert_eq!(out.status.code(), Some(0), "{:?}", out.status);
        assert_eq!(out.stdout, b"x\xEF\xBF\xBDy\n");
    }
}

/// GIVEN a program that prints each of its arguments
/// AND the argument `two words`
/// WHEN it runs with that argument, with and without `-O2`
/// THEN it exits 0 and prints `two words` as one line
///
/// An argument holding a space, which a `// args:` line cannot pass, stays one argument.
#[test]
fn argument_with_a_space_should_stay_one_argument() {
    for o2 in [false, true] {
        let exe = compile_tmp("echo_args_spaces", ECHO_ARGS, o2);
        let out = support::Invocation::new(Vec::new(), vec!["two words".into()]).run(&exe, false);
        assert_eq!(out.status.code(), Some(0), "{:?}", out.status);
        assert_eq!(String::from_utf8_lossy(&out.stdout), "two words\n");
    }
}

/// GIVEN a program that echoes its input
/// AND input of several buffers, with a line longer than one buffer
/// WHEN it runs under Valgrind, with and without `-O2`
/// THEN it exits 0 and prints the input unchanged
///
/// A line longer than the runtime's 8 KiB input buffer, input of several buffers, and a `€`
/// at bytes 8191..8194 (across the first buffer's end when the first read fills it) all come
/// back whole, with no memory error. The `Lines` unit tests force the split itself.
#[test]
fn read_line_should_return_long_lines_whole_across_buffer_refills() {
    let mut first = "a".repeat(8191) + "€";
    first += &"b".repeat(20_000 - first.len());
    let mut input = first.into_bytes();
    input.push(b'\n');
    input.extend(generated_lines(40_000 - input.len()));
    assert_eq!(&input[8191..8194], "€".as_bytes());
    for o2 in [false, true] {
        let exe = compile_tmp_for_valgrind("echo_refill", ECHO, o2);
        let out = support::Invocation::new(input.clone(), Vec::new()).run(&exe, true);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{stderr}");
        assert!(out.stdout == input, "{stderr}");
    }
}

/// GIVEN a line of 2 MiB that holds a million words
/// WHEN two programs read it, one that keeps `line.words()` in a local and one that counts it, with
/// and without `-O2`
/// THEN both exit 0 and print 1048576
///
/// A line of 2 MiB holds a million words, each found from where the last one ended. A split
/// that rescanned from the start of the line would take far longer than the test's timeout.
/// Both forms run: a `[string]` kept in a local is built word by word, and
/// `line.words().len()` only counts.
#[test]
fn words_should_count_a_million_words_in_a_two_mebibyte_line() {
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

/// GIVEN a program that echoes its input
/// AND 1 MiB of input, which is more than a pipe holds
/// WHEN it runs, with and without `-O2`
/// THEN it exits 0 and prints the input unchanged
///
/// More input than a pipe holds reaches the program while the harness reads its output, so
/// neither side waits on the other.
#[test]
fn stdin_larger_than_a_pipe_should_come_back_whole() {
    let input = generated_lines(1 << 20);
    for o2 in [false, true] {
        let exe = compile_tmp("echo_large", ECHO, o2);
        let out = support::Invocation::new(input.clone(), Vec::new()).run(&exe, false);
        assert_eq!(out.status.code(), Some(0), "{:?}", out.status);
        assert!(out.stdout == input, "{} bytes out", out.stdout.len());
    }
}

/// GIVEN a program that reads one line and ends
/// AND 1 MiB of input
/// WHEN it runs, with and without `-O2`
/// THEN it exits 0 and prints the first line
///
/// A program that exits before it reads all its input ends normally; the harness ignores
/// the failed write of the rest.
#[test]
fn program_exiting_before_its_input_ends_should_exit_normally() {
    let source = "fn main() {\n    print(read_line() ?? \"none\")\n}\n";
    let input = generated_lines(1 << 20);
    for o2 in [false, true] {
        let exe = compile_tmp("read_one_line", source, o2);
        let out = support::Invocation::new(input.clone(), Vec::new()).run(&exe, false);
        assert_eq!(out.status.code(), Some(0), "{:?}", out.status);
        assert_eq!(String::from_utf8_lossy(&out.stdout), "line \n");
    }
}

/// GIVEN a program that calls `read_line`
/// AND standard input is a directory
/// WHEN it is built and run, with and without `-O2`
/// THEN it exits 101 and writes only a panic line that has no location
///
/// A failed read (`EISDIR`) is a panic with no location.
#[test]
fn read_line_should_panic_without_a_location_when_stdin_is_a_directory() {
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

/// GIVEN a program that prints `read_line() ?? "none"`
/// AND standard input is closed
/// WHEN it is built and run, with and without `-O2`
/// THEN it exits 0 and prints `none`
///
/// A closed standard input is the end of input.
#[test]
fn read_line_should_return_none_when_stdin_is_closed() {
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

/// GIVEN a program that prints 1 and leaks a cycle
/// AND it is built with `--rc-checks`
/// WHEN it runs with standard output and standard error on one pipe
/// THEN it exits 102 and the pipe holds `1` and then the live-object line
///
/// An `--rc-checks` program that leaks exits 102 and still delivers what it printed.
#[test]
fn rc_checks_exit_should_flush_stdout() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("rc_flush.sisu"), LEAKS_A_CYCLE).expect("writes");
    let exe = dir.join("rc_flush");
    support::compile(dir, "rc_flush", &exe, &["--rc-checks"]);
    let (status, merged) = run_with_merged_output(&exe);
    assert_eq!(status.code(), Some(102), "{status:?}");
    assert_eq!(merged, "1\nsisu: rc check: live objects at exit: 1\n");
}

/// GIVEN a program that prints 3000 lines and then dies by a stack overflow
/// AND standard output is a pipe
/// WHEN it runs
/// THEN it dies by signal 11 and the pipe holds whole lines, in order, of at least 8192 bytes but
/// not all
///
/// Output goes out in blocks while the program runs: a program that prints 3000 lines and
/// then dies by a stack overflow leaves whole blocks on a pipe and loses only the tail.
#[test]
fn full_blocks_of_output_should_survive_a_stack_overflow() {
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

/// GIVEN two programs that print 1 line and 20 000 lines
/// WHEN each runs under Valgrind
/// THEN both make the same number of allocations
///
/// `print` collects its output in one buffer, however much the program prints.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "reads Valgrind's output, and Valgrind runs only on Linux"
)]
fn print_should_allocate_one_buffer_however_much_it_prints() {
    let program = |limit: u32| {
        format!(
            "fn main() {{\n    let mut i = 0\n    while i < {limit} {{\n        print(i)\n        i += 1\n    }}\n}}\n"
        )
    };
    let (_, one) = heap_allocs(&compile_tmp_for_valgrind(
        "one_buffer_k1",
        &program(1),
        false,
    ));
    let (_, many) = heap_allocs(&compile_tmp_for_valgrind(
        "one_buffer_k20000",
        &program(20_000),
        false,
    ));
    assert_eq!(many, one);
}

/// GIVEN a program that prints 100 000 numbers
/// AND standard output is a file
/// WHEN it is built and run, with and without `-O2`
/// THEN it exits 0 and the file holds every number, in order
///
/// Enough output to fill the buffer many times arrives whole and in order.
#[test]
fn long_output_to_a_file_should_arrive_whole_and_in_order() {
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
const LEAKS_A_CYCLE: &str = "class Node {\n    mut _next: Node?\n}\n\nfn leak() {\n    let a = Node(_next: None)\n    a._next = a\n}\n\nfn main() {\n    leak()\n    print(1)\n}\n";

/// GIVEN a program that leaks a cycle and prints 1
/// WHEN it runs plain and then under Valgrind
/// THEN the plain run exits 0 and prints 1
/// AND Valgrind exits 1 and reports a lost block
///
/// Proves the harness catches a leak.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "reads Valgrind's output, and Valgrind runs only on Linux"
)]
fn valgrind_run_should_report_a_leaked_cycle() {
    let exe = compile_tmp_for_valgrind("cycle", LEAKS_A_CYCLE, false);
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

/// GIVEN a build that links mimalloc, a stripped build and a system build
/// WHEN each runs under Valgrind
/// THEN the first two panic with a message that names the cause
/// AND the system build exits 0 and prints the symbol name
///
/// The Valgrind runs refuse a build that links mimalloc, whose blocks Valgrind cannot see, and a
/// stripped build, whose allocator they cannot tell. The guard reads symbols, not bytes: the
/// program prints the name of the symbol it looks for, and its system build is accepted.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "the guard runs with Valgrind, and Valgrind runs only on Linux"
)]
fn valgrind_runs_should_refuse_a_mimalloc_build_and_a_stripped_build() {
    const PROGRAM: &str = "fn main() {\n    print(\"mi_malloc_aligned\")\n}\n";
    // Every build before any `panic_message`, so a failed compile is not taken for the guard.
    let default = compile_tmp("refuse_default", PROGRAM, false);
    let system = compile_tmp_for_valgrind("refuse_system", PROGRAM, false);
    let stripped = Path::new(env!("CARGO_TARGET_TMPDIR")).join("refuse_stripped");
    fs::copy(&system, &stripped).expect("copies the system build");
    let status = Command::new("strip")
        .arg(&stripped)
        .status()
        .expect("strip starts");
    assert!(status.success(), "strip: {status:?}");
    let message = panic_message(|| {
        support::Invocation::default().run(&default, true);
    });
    assert!(message.contains("--allocator system"), "{message}");
    let message = panic_message(|| {
        support::Invocation::default().run(&stripped, true);
    });
    assert!(message.contains("sisu_alloc"), "{message}");
    let out = support::Invocation::default().run(&system, true);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "mi_malloc_aligned\n");
}

/// GIVEN a build with no `--allocator` flag and a system build
/// WHEN `require_mimalloc` checks each
/// THEN it accepts the first and panics on the second with a message that names the cause
///
/// The `tests/run` harness uses it on its `-O2` build on mimalloc.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "macOS symbol names have a leading `_`, so the check runs on Linux only"
)]
fn require_mimalloc_should_accept_a_default_build_and_refuse_a_system_build() {
    const PROGRAM: &str = "fn main() {\n    print(1)\n}\n";
    let default = compile_tmp("require_default", PROGRAM, true);
    let system = compile_tmp_for_valgrind("require_system", PROGRAM, true);
    support::require_mimalloc(&default);
    let message = panic_message(|| support::require_mimalloc(&system));
    assert!(message.contains("does not link mimalloc"), "{message}");
}

/// GIVEN a `run` case that passes
/// WHEN the harness runs it
/// THEN the case directory holds the `-O2` executable on mimalloc, and it links mimalloc
///
/// Without the build, the harness runs no `-O2` code on the allocator that users get. The harness
/// also runs the executable and compares its output, but the program cannot see which build runs
/// it, so only the executable's existence and allocator are checked, with no test-only seam in
/// `run`.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "macOS symbol names have a leading `_`, so the check runs on Linux only"
)]
fn run_should_build_the_o2_executable_on_mimalloc() {
    let dir = fixture(
        "run_builds_o2_mimalloc",
        &[("a.sisu", PRINTS_1), ("a.out", "1\n")],
    );
    let source = dir.join("a.sisu");
    support::run(&source, false);
    let exe = support::case_path(&source).join(support::MIMALLOC_O2.0);
    assert!(exe.exists(), "{} should exist", exe.display());
    support::require_mimalloc(&exe);
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

/// GIVEN a program that leaks a cycle and prints 1
/// AND it is built with `--rc-checks`
/// WHEN it runs, with and without `-O2`
/// THEN it exits 102, prints 1, and reports one live object at exit
///
/// The cycle Valgrind reports, `--rc-checks` reports too, with or without `-O2`: one object is
/// live at exit.
#[test]
fn rc_checks_should_report_a_leaked_cycle() {
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

/// GIVEN a program that holds an object and then divides by zero
/// AND it is built with `--rc-checks`
/// WHEN it runs
/// THEN it exits 101 with the panic line and no live-object report
///
/// A panic exits 101 without releasing, so the object `b` still holds is no leak to report.
#[test]
fn rc_checks_should_not_report_a_leak_when_the_program_panics() {
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

/// GIVEN a program that divides by zero
/// AND standard error is `/dev/full`
/// WHEN it is built and run
/// THEN it exits 101
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "writes to /dev/full, which only Linux has"
)]
fn panic_should_exit_101_when_stderr_is_full() {
    // Writes to /dev/full fail with ENOSPC, so the runtime's panic line cannot be written.
    let full = File::options()
        .write(true)
        .open("/dev/full")
        .expect("/dev/full exists on Linux");
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

/// GIVEN a source file `overwrite_input.sisu`
/// WHEN `sisu build` gets that file, or `./overwrite_input.sisu`, as the output path
/// THEN it exits 1 and says the output would overwrite the input
/// AND the source file is unchanged
#[test]
fn build_should_refuse_an_output_that_names_the_input() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let source = "fn main() {\n    print(1)\n}\n";
    let input = dir.join("overwrite_input.sisu");
    fs::write(&input, source).expect("writes");
    for output in ["overwrite_input.sisu", "./overwrite_input.sisu"] {
        let out = support::sisu(dir, "overwrite_input.sisu", Path::new(output), &[])
            .output()
            .expect("sisu starts");
        assert_eq!(out.status.code(), Some(1), "output {output}");
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            format!("sisu: output {output} would overwrite the input\n")
        );
        assert_eq!(fs::read_to_string(&input).expect("reads"), source);
    }
}

/// GIVEN a file `my prog` that has no extension, and a file `my prog.sisu`
/// WHEN `sisu build` runs on each without `-o`
/// THEN the first build exits 1 and leaves the file as it was
/// AND the second build makes the executable `my prog`, which prints 1
///
/// Without `-o`, a file with no extension would be built over itself, and a stem with a space
/// names the executable whole.
#[test]
fn build_should_name_the_executable_after_the_whole_file_stem() {
    let dir = empty_dir("build_whole_stem");
    let source = "fn main() {\n    print(1)\n}\n";
    let build = |input: &str| {
        Command::new(env!("CARGO_BIN_EXE_sisu"))
            .current_dir(&dir)
            .args(["build", input])
            .output()
            .expect("sisu starts")
    };
    fs::write(dir.join("my prog"), source).expect("writes");
    let out = build("my prog");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "sisu: output my prog would overwrite the input\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("my prog")).expect("reads"),
        source
    );
    fs::remove_file(dir.join("my prog")).expect("removes");
    fs::write(dir.join("my prog.sisu"), source).expect("writes");
    let out = build("my prog.sisu");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(dir.join("my prog"))
        .output()
        .expect("the program starts");
    assert_eq!(String::from_utf8_lossy(&run.stdout), "1\n");
}

/// Builds the `tests/run` case `loops` with `flags` and `PATH`, runs it, and checks its output.
/// Returns the executable's bytes.
fn build_loops(exe: &str, flags: &[&str], path: &OsStr) -> Vec<u8> {
    let exe = Path::new(env!("CARGO_TARGET_TMPDIR")).join(exe);
    let out = support::sisu(Path::new(RUN), "loops.sisu", &exe, flags)
        .env("PATH", path)
        .output()
        .expect("sisu starts");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&exe).output().expect("the program starts");
    assert_eq!(
        run.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        fs::read_to_string(Path::new(RUN).join("loops.out")).expect("reads the expected output"),
        "stdout should match loops.out"
    );
    fs::read(&exe).expect("reads the executable")
}

/// GIVEN the `loops` case of `tests/run`
/// WHEN it is built with `-O3` and run
/// THEN it exits 0 and prints the contents of `loops.out`
#[test]
fn o3_build_should_make_a_program_that_runs() {
    build_loops(
        "loops-O3",
        &["-O3"],
        &std::env::var_os("PATH").unwrap_or_default(),
    );
}

/// GIVEN the `loops` case of `tests/run` and `ld.lld` on `PATH`
/// WHEN it is built with `--linker lld` and with `--linker bfd`
/// THEN only the first executable records `Linker: LLD`
///
/// `--linker lld` links with LLVM's `ld.lld`, which records itself in the `.comment` section;
/// GNU ld, which `--linker bfd` picks, does not. `cc`'s default linker varies by host, so the
/// test names both.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "reads the ELF `.comment` section and GNU ld"
)]
fn linker_flag_should_pick_the_named_linker() {
    let bin = Path::new(env!("LLVM_SYS_231_PREFIX")).join("bin");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&path)))
        .expect("PATH joins");
    let lld = build_loops("loops-lld", &["--linker", "lld"], &path);
    assert!(lld.windows(11).any(|w| w == b"Linker: LLD"));
    let bfd = build_loops("loops-bfd", &["--linker", "bfd"], &path);
    assert!(!bfd.windows(11).any(|w| w == b"Linker: LLD"));
}

/// GIVEN the `loops` case of `tests/run`
/// AND `PATH` holds only `cc` and `COMPILER_PATH` holds `ld.lld`
/// WHEN it is built with `--linker lld`
/// THEN it exits 0 and the executable records `Linker: LLD`
///
/// gcc finds a linker in `COMPILER_PATH` too, so `--linker` does not require it on `PATH`.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "links with `--linker lld`, and Apple's clang looks for `ld64.lld`, which `.llvm/23` lacks"
)]
fn linker_flag_should_work_when_the_linker_is_only_in_compiler_path() {
    let exe = Path::new(env!("CARGO_TARGET_TMPDIR")).join("loops-compiler-path");
    let out = support::sisu(Path::new(RUN), "loops.sisu", &exe, &["--linker", "lld"])
        .env("PATH", only_cc("only_cc_compiler_path"))
        .env(
            "COMPILER_PATH",
            Path::new(env!("LLVM_SYS_231_PREFIX")).join("bin"),
        )
        .output()
        .expect("sisu starts");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let executable = fs::read(&exe).expect("reads the executable");
    assert!(executable.windows(11).any(|w| w == b"Linker: LLD"));
}

/// GIVEN the `loops` case of `tests/run`
/// AND `PATH` holds only `cc`
/// WHEN it is built with `--linker mold`
/// THEN it exits 1 and the first stderr line names the linker
/// AND the output directory stays empty
///
/// With only `cc` on `PATH`, `cc` runs and fails to find `ld.mold` (`ld64.mold` on macOS);
/// `sisu` names it.
#[test]
fn build_should_name_a_linker_that_is_missing_from_path() {
    let empty = empty_dir("no_linker_on_path");
    let exe = empty.join("loops");
    let out = support::sisu(Path::new(RUN), "loops.sisu", &exe, &["--linker", "mold"])
        .env("PATH", only_cc("only_cc_on_path"))
        .env_remove("COMPILER_PATH")
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(1));
    // cc's own message follows on the next lines.
    let linker = if cfg!(target_os = "macos") {
        "ld64.mold"
    } else {
        "ld.mold"
    };
    assert_eq!(
        String::from_utf8_lossy(&out.stderr).lines().next(),
        Some(
            format!("sisu: linker {linker} not found on PATH; cc failed: exit status: 1").as_str()
        )
    );
    assert_empty(&empty);
}

fn assert_empty(dir: &Path) {
    let left: Vec<_> = fs::read_dir(dir).expect("reads").collect();
    assert!(left.is_empty(), "sisu left {left:?}");
}

/// GIVEN a source file and an object file of the same stem in the same directory
/// WHEN `sisu build` runs with an empty `TMPDIR`
/// THEN it exits 0 and the object file is unchanged
/// AND `TMPDIR` is empty afterwards
#[test]
fn build_should_keep_an_existing_object_file_next_to_the_source() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("keep_object.sisu"), "fn main() {}\n").expect("writes");
    fs::write(dir.join("keep_object.o"), "not sisu's").expect("writes");
    let tmp = empty_dir("keep_object_tmp");
    let out = support::sisu(dir, "keep_object.sisu", &dir.join("keep_object"), &[])
        .env("TMPDIR", &tmp)
        .output()
        .expect("sisu starts");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(dir.join("keep_object.o")).expect("`keep_object.o` is still there"),
        "not sisu's"
    );
    assert_empty(&tmp);
}

/// GIVEN a source file
/// AND a linker that `cc` cannot find
/// WHEN `sisu build` runs with an empty `TMPDIR`
/// THEN it exits 1 and says that `cc` failed
/// AND no object file is left in `TMPDIR` or next to the source
#[test]
fn failed_link_should_leave_no_object_file() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("link_fails.sisu"), "fn main() {}\n").expect("writes");
    // With only `cc` on `PATH`, `cc` fails to find `ld.mold`, for any user.
    let output = empty_dir("link_fails").join("out");
    let tmp = empty_dir("link_fails_tmp");
    let out = support::sisu(dir, "link_fails.sisu", &output, &["--linker", "mold"])
        .env("TMPDIR", &tmp)
        .env("PATH", only_cc("link_fails_path"))
        .env_remove("COMPILER_PATH")
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("cc failed: "), "{stderr}");
    assert_empty(&tmp);
    assert!(!dir.join("link_fails.o").exists());
}

/// Runs `sisu build <name>.sisu -o <name>` on `source` in the target tmp dir, after removing any
/// earlier `<name>`; returns the output and the executable's path.
fn build_tmp(name: &str, source: &str) -> (Output, PathBuf) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join(format!("{name}.sisu")), source).expect("writes");
    let exe = dir.join(name);
    // It is absent on the first run, so a failure here is expected and harmless.
    drop(fs::remove_file(&exe));
    let out = support::sisu(dir, &format!("{name}.sisu"), &exe, &[])
        .output()
        .expect("sisu starts");
    (out, exe)
}

/// GIVEN a program that assigns to an immutable binding
/// WHEN `sisu build` runs on it
/// THEN it exits 1 and reports error `E0001`
/// AND no executable exists
#[test]
fn build_error_should_make_no_executable() {
    let (out, exe) = build_tmp(
        "build_error_makes_no_executable",
        "fn main() {\n    let n = 0\n    n = 1\n}\n",
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("error[E0001]: cannot assign to `n`"));
    assert!(!exe.exists());
}

/// GIVEN a program that has a `mut` binding that is never reassigned
/// WHEN `sisu build` runs on it
/// THEN it exits 0 and reports warning `W0002`
/// AND the executable runs and prints 0
#[test]
fn build_warning_should_still_make_the_executable() {
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

/// Runs `f`, which should panic, and returns its panic message.
fn panic_message(f: impl FnOnce()) -> String {
    let payload = panic::catch_unwind(AssertUnwindSafe(f)).expect_err("the call should panic");
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .expect("a panic message should be a string")
}

/// GIVEN a `run` case that has no `.out` file
/// WHEN the harness runs it in bless mode
/// THEN it panics and says to create the file empty
/// AND it creates no `.out` file
#[test]
fn bless_should_refuse_to_create_a_missing_out_file() {
    let dir = fixture(
        "bless_refuses_to_create_a_missing_out",
        &[("a.sisu", PRINTS_1)],
    );
    let message = panic_message(|| support::run(&dir.join("a.sisu"), true));
    assert!(message.contains("is missing; create it empty"), "{message}");
    assert!(!dir.join("a.out").exists());
}

/// GIVEN a `run` case that panics, with an empty `.out` file
/// WHEN the harness runs it in bless mode
/// THEN it panics and names the exit code 101
/// AND the `.out` file stays empty
#[test]
fn bless_should_not_write_after_a_wrong_exit_code() {
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

/// GIVEN a case with the marker `o2-calls: main one 1`, where `-O2` inlines `one`
/// WHEN the harness runs it
/// THEN it panics and says that `-O2` keeps 0 calls to `one` in `main`
///
/// An `o2-calls` marker fails the case when the inliner folds the call away.
#[test]
fn o2_calls_marker_should_fail_when_the_inliner_folds_the_call_away() {
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

/// GIVEN a case where `main` calls `outer` twice and `outer` calls `down(5)` once
/// WHEN the harness runs it with the marker `outer down 1` and then `outer down 2`
/// THEN the first run passes
/// AND the second run panics and says that `-O2` keeps 1 calls to `down` in `outer`
///
/// A marker counts the calls in the caller's own body: `outer` keeps one call to `down`, not
/// the two that its inlined copies in `main` add up to.
#[test]
fn o2_calls_marker_should_count_calls_in_the_callers_own_body() {
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

/// GIVEN a case with the markers `main outer 2` and `outer down 1`
/// WHEN the harness runs it
/// THEN it panics and says that `-O2` keeps 0 calls to `outer` in `main`
///
/// Each caller gets a build of its own. Keeping `outer` for its marker must not keep it for
/// `main`'s: a real build inlines `outer`, so `main` keeps no call to it.
#[test]
fn o2_calls_marker_should_keep_one_caller_at_a_time() {
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

/// GIVEN a case whose marker names a caller that takes parameters
/// WHEN the harness runs it
/// THEN it panics and says that the caller takes parameters
///
/// With `outer` kept as a function of its own, `outer(n)` holds a call to `down` that its real
/// call sites decide away, so a caller with parameters is rejected.
#[test]
fn o2_calls_marker_should_reject_a_caller_with_parameters() {
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

/// GIVEN a case with the marker `main down 0`
/// WHEN the harness runs it
/// THEN it panics and says that the marker proves no call boundary
///
/// `down(0)` folds to a constant, so a count of 0 holds and proves nothing; it is rejected.
#[test]
fn o2_calls_marker_should_reject_a_count_of_zero() {
    let dir = o2_case(
        "o2_calls_marker_rejects_a_count_of_zero",
        "main down 0",
        "fn main() {\n    print(down(0))\n}\n",
        "0\n",
    );
    let message = panic_message(|| support::run(&dir.join("a.sisu"), false));
    assert!(message.contains("proves no call boundary"), "{message}");
}

/// GIVEN a case with two `// args:` lines
/// WHEN the harness reads its invocation
/// THEN it panics and says that the case has more than one `// args:` line
///
/// A case has one argument list, so a second `// args:` line is a mistake.
#[test]
#[should_panic(expected = "more than one `// args:` line")]
fn case_should_fail_when_it_has_two_args_lines() {
    let dir = fixture(
        "two_args_markers_fail",
        &[("a.sisu", "// args: a\n// args: b\nfn main() {}\n")],
    );
    support::Invocation::of(&dir.join("a.sisu"));
}

/// GIVEN a case whose marker has a misspelled caller, and a case whose marker has a misspelled
/// callee
/// WHEN the harness runs each
/// THEN each panics and says that there is no function with that name
#[test]
fn o2_calls_marker_should_name_a_function_that_does_not_exist() {
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

/// GIVEN a case with the marker `o2-calls: main one`, which has no count
/// WHEN the harness runs it
/// THEN it panics and says that the marker is not `<caller> <callee> <count>`
#[test]
fn o2_calls_marker_should_be_rejected_without_a_count() {
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

/// GIVEN a `run` case that leaks a cycle, with a wrong `.out` file
/// WHEN the harness runs it in bless mode
/// THEN it panics with the Valgrind leak summary
/// AND the `.out` file is unchanged
///
/// A run that passes natively but fails under Valgrind writes nothing.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "reads Valgrind's output, and Valgrind runs only on Linux"
)]
fn bless_should_not_write_after_a_valgrind_failure() {
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

/// GIVEN a `run_fail` case that has an `.out` file and no `.err` file
/// WHEN the harness runs it in bless mode
/// THEN it panics and says that the `.err` file is missing
/// AND the `.out` file is unchanged and no `.err` file exists
///
/// Bless writes a `run_fail` case's files all or nothing: a missing `.err` leaves `.out` as it was.
#[test]
fn bless_should_write_neither_file_when_one_is_missing() {
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

/// GIVEN a diagnostics case whose program reports no diagnostic, with an empty `.err` file
/// WHEN the harness runs it in bless mode
/// THEN it panics and says that the program reports no diagnostic
/// AND the `.err` file stays empty
///
/// A program that reports nothing is no diagnostics case, whatever its `.err` holds.
#[test]
fn diagnostics_case_should_be_rejected_when_the_program_reports_nothing() {
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

/// GIVEN a `run` case with a wrong `.out` file
/// WHEN the harness runs it without bless and then with bless
/// THEN the first run panics and leaves the file as it was
/// AND the second run writes the right output, and a run without bless then passes
///
/// Without bless a wrong `.out` fails and stays as it was; bless rewrites it, and the run then
/// passes without bless.
#[test]
fn bless_should_rewrite_a_wrong_out_file() {
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

/// GIVEN a `run` case that writes to standard error, with a wrong `.err` file, and the same program
/// with no `.err` file
/// WHEN the harness runs the first without bless, with bless, and without bless again, and runs the
/// second
/// THEN the first run panics and leaves the file, then bless writes the right text and the next run
/// passes
/// AND the second case panics and says that it has no `.err` file
///
/// A `run` case that writes to standard error needs its `.err`: a wrong one fails and stays as it
/// was, bless rewrites it, and the run then passes. Without one the stderr must be empty.
#[test]
fn run_case_should_check_and_bless_its_err_file() {
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

/// GIVEN a directory with the case `a` and `ghost.out`, which has no `ghost.sisu`
/// WHEN `orphans` lists the directory
/// THEN it returns one message, which names `ghost.out`
#[test]
fn orphans_should_list_an_out_file_without_a_source() {
    let dir = fixture(
        "orphan_out_fails",
        &[("a.sisu", PRINTS_1), ("a.out", "1\n"), ("ghost.out", "1\n")],
    );
    let messages = support::orphans(&dir, &["out"]);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("ghost.out"), "{messages:?}");
}

/// GIVEN a `run` case with a file `notes.txt` beside it
/// WHEN `orphans` lists the directory
/// THEN it returns one message, which names `notes.txt`
#[test]
fn orphans_should_list_a_file_of_an_unknown_type() {
    let dir = fixture(
        "stray_file_type_fails",
        &[("notes.sisu", PRINTS_1), ("notes.txt", "")],
    );
    let messages = support::orphans(&dir, &support::RUN_EXPECTATIONS);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("notes.txt"), "{messages:?}");
}

/// GIVEN a `run` case with a `.json` file beside it
/// WHEN `orphans` lists the directory
/// THEN it returns one message, which names `a.json`
///
/// A `.json` beside a `run` case is never read, so it is listed like an orphan.
#[test]
fn orphans_should_list_a_json_file_beside_a_run_case() {
    let dir = fixture(
        "expectation_of_another_mode_fails",
        &[("a.sisu", PRINTS_1), ("a.out", "1\n"), ("a.json", "")],
    );
    let messages = support::orphans(&dir, &support::RUN_EXPECTATIONS);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("a.json"), "{messages:?}");
}

/// GIVEN a `run` case with a hidden file `.ghost.out` beside it
/// WHEN `orphans` lists the directory
/// THEN it returns one message, which names `.ghost.out`
///
/// The harness never lists a hidden file, so only the listing of the directory finds it.
#[test]
fn orphans_should_list_a_hidden_file_beside_a_case() {
    let dir = fixture(
        "hidden_file_fails",
        &[
            ("a.sisu", PRINTS_1),
            ("a.out", "1\n"),
            (".ghost.out", "1\n"),
        ],
    );
    let messages = support::orphans(&dir, &["out"]);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains(".ghost.out"), "{messages:?}");
}

/// GIVEN a directory with the case `a`, a file `notes.txt` and a hidden file `.ghost.out`
/// WHEN `orphans` lists the directory
/// THEN it returns two messages, and they name `.ghost.out` and `notes.txt`
#[test]
fn orphans_should_list_every_bad_entry_of_a_directory() {
    let dir = fixture(
        "every_bad_entry_listed",
        &[
            ("a.sisu", PRINTS_1),
            ("a.out", "1\n"),
            ("notes.txt", ""),
            (".ghost.out", "1\n"),
        ],
    );
    let messages = support::orphans(&dir, &["out"]);
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(messages[0].contains(".ghost.out"), "{messages:?}");
    assert!(messages[1].contains("notes.txt"), "{messages:?}");
}

/// GIVEN a directory with the case `a` and a subdirectory `sub`
/// WHEN `orphans` lists the directory
/// THEN it returns one message, which names `sub` as a directory
///
/// A case directory is flat, so a case inside `sub` would go unchecked.
#[test]
fn orphans_should_reject_a_subdirectory() {
    let dir = fixture(
        "subdirectory_rejected",
        &[
            ("a.sisu", PRINTS_1),
            ("a.out", "1\n"),
            ("sub/b.sisu", PRINTS_1),
        ],
    );
    let messages = support::orphans(&dir, &["out"]);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("sub is a directory"), "{messages:?}");
}

/// GIVEN a directory with `a.out` and no `.sisu` file
/// WHEN `orphans` lists the directory
/// THEN it panics and says that the directory holds no case
///
/// A wrong path would otherwise list no entry and pass.
#[test]
#[should_panic(expected = "holds no `.sisu` case")]
fn orphans_should_fail_for_a_directory_with_no_case() {
    let dir = fixture("directory_with_no_case", &[("a.out", "1\n")]);
    support::orphans(&dir, &["out"]);
}

/// GIVEN cases named `dup` in `run`, in `run-fail`, in `run/a_b` and in `run/a/b`
/// WHEN the harness runs them at the same time
/// THEN every case passes
///
/// Same-named cases in different directories, run at once, must not share an executable: one
/// in each mode, and two whose paths differ only in `a_b` against `a/b`.
#[test]
fn same_named_cases_should_not_share_an_executable_when_run_at_once() {
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

/// GIVEN the programs `life`, `optionals` and `arrays`
/// WHEN each is built twice with `-O2`, at two different output paths
/// THEN the two executables of each program have the same bytes
///
/// Each program builds to the same bytes twice, at two different output paths. Their file names
/// match, because macOS's linker signs each executable under its file name.
#[test]
fn builds_should_give_the_same_bytes_at_two_output_paths() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    for name in ["life", "optionals", "arrays"] {
        let [a, b] = ["a", "b"].map(|build| {
            let build_dir = dir.join(format!("deterministic-{build}"));
            fs::create_dir_all(&build_dir).expect("creates the build directory");
            let exe = build_dir.join(name);
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

    /// GIVEN generated `i64`, `u8` and `bool` expressions from four seeds
    /// WHEN one program that prints them is built and run, with and without `-O2`
    /// THEN it exits 0 and prints the line that Rust computes for each expression
    ///
    /// Fifty expressions per seed, each regenerated until Rust can evaluate it, printed by one
    /// program, which both builds must run to exactly the lines Rust computes.
    #[test]
    fn generated_arithmetic_should_print_what_rust_computes() {
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

/// GIVEN the stderr of a build whose source quotes `warning[W0012]` in a string, under a W0002
/// AND the stderr of a build with a real W0012
/// WHEN the run harness looks for an unused-name warning in each
/// THEN it finds none in the first and one in the second
#[test]
fn unused_warning_should_match_only_a_diagnostic_first_line() {
    let quoted = "warning[W0002]: `n` is never reassigned\n --> fib.sisu:6:13\n  |\n6 |     let mut n = 30; let _ = \"warning[W0012]\"\n  |             ^\n  |\nhelp: declare it with `let`\n  |\n6 -     let mut n = 30; let _ = \"warning[W0012]\"\n6 +     let n = 30; let _ = \"warning[W0012]\"\n  |\n\nsisu: 1 warning\nsisu: for more on a code, run `sisu explain <code>`: W0002\n";
    let real = "warning[W0012]: function `probe` is never called\n --> fib.sisu:4:4\n  |\n4 | fn probe() {}\n  |    ^^^^^ never called\n  |\n\nsisu: 1 warning\nsisu: for more on a code, run `sisu explain <code>`: W0012\n";
    assert!(!support::has_unused_warning(quoted));
    assert!(support::has_unused_warning(real));
}

/// The file of every token kind and every case of the lexer's rules.
const ALL_TOKENS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/tokens/all_tokens.sisu");

/// Compares the lexer in Sisu with `sisu build --emit tokens` on `input`, which goes into a file
/// of its own for the test `test`.
fn compare_text(test: &str, input: &str) {
    let dir = fixture(test, &[("input.sisu", input)]);
    selfhost::compare(&dir.join("input.sisu"));
}

/// GIVEN `tests/tokens/all_tokens.sisu`, which holds every token kind and each case of the rules
/// WHEN each build of `selfhost/lexer.sisu` reads it on standard input
/// THEN each prints the tokens that `sisu build --emit tokens` prints for it, byte for byte
#[test]
fn selfhost_lexer_should_match_emit_tokens_on_all_tokens() {
    selfhost::compare(Path::new(ALL_TOKENS));
}

/// GIVEN `all_tokens.sisu` with each `\n` made `\r\n`
/// WHEN each build of the lexer reads it
/// THEN the tokens are those of `sisu build --emit tokens`, and each `Newline` sits on its `\r`
#[test]
fn selfhost_lexer_should_match_emit_tokens_on_all_tokens_with_carriage_returns() {
    let text = fs::read_to_string(ALL_TOKENS).expect("all_tokens.sisu should be readable");
    compare_text(
        "selfhost_lexer_should_match_emit_tokens_on_all_tokens_with_carriage_returns",
        &text.replace('\n', "\r\n"),
    );
}

/// GIVEN `all_tokens.sisu` without its final line break
/// WHEN each build of the lexer reads it
/// THEN the tokens are those of `sisu build --emit tokens`, and the last `Newline` follows the `}`
#[test]
fn selfhost_lexer_should_match_emit_tokens_on_all_tokens_without_a_final_line_break() {
    let text = fs::read_to_string(ALL_TOKENS).expect("all_tokens.sisu should be readable");
    compare_text(
        "selfhost_lexer_should_match_emit_tokens_on_all_tokens_without_a_final_line_break",
        text.strip_suffix('\n')
            .expect("all_tokens.sisu should end with a line break"),
    );
}

/// GIVEN the input `x\r`, a `\r` that ends the input
/// WHEN each build of the lexer reads it
/// THEN the `Newline` is at 1:3, after the `\r`, as in `sisu build --emit tokens`
#[test]
fn selfhost_lexer_should_match_emit_tokens_on_a_carriage_return_at_the_end() {
    compare_text(
        "selfhost_lexer_should_match_emit_tokens_on_a_carriage_return_at_the_end",
        "x\r",
    );
}

/// GIVEN the input `x\r\n`, a `\r` before a line feed
/// WHEN each build of the lexer reads it
/// THEN the `Newline` is at 1:2, on the `\r`, as in `sisu build --emit tokens`
#[test]
fn selfhost_lexer_should_match_emit_tokens_on_a_carriage_return_and_a_line_feed() {
    compare_text(
        "selfhost_lexer_should_match_emit_tokens_on_a_carriage_return_and_a_line_feed",
        "x\r\n",
    );
}

/// GIVEN the input `x\r\r\n`, two `\r` before a line feed
/// WHEN each build of the lexer reads it
/// THEN the `Newline` is at 1:3, on the second `\r`, as in `sisu build --emit tokens`
#[test]
fn selfhost_lexer_should_match_emit_tokens_on_two_carriage_returns_and_a_line_feed() {
    compare_text(
        "selfhost_lexer_should_match_emit_tokens_on_two_carriage_returns_and_a_line_feed",
        "x\r\r\n",
    );
}

/// GIVEN the input `a\rb\n`, a `\r` inside a line
/// WHEN each build of the lexer reads it
/// THEN `b` is at 1:3 and the `Newline` at 1:4, as in `sisu build --emit tokens`
#[test]
fn selfhost_lexer_should_match_emit_tokens_on_a_carriage_return_inside_a_line() {
    compare_text(
        "selfhost_lexer_should_match_emit_tokens_on_a_carriage_return_inside_a_line",
        "a\rb\n",
    );
}

/// GIVEN an empty input
/// WHEN each build of the lexer reads it
/// THEN it prints `1:1 Eof` only, as `sisu build --emit tokens` does
#[test]
fn selfhost_lexer_should_match_emit_tokens_on_an_empty_input() {
    compare_text(
        "selfhost_lexer_should_match_emit_tokens_on_an_empty_input",
        "",
    );
}

/// GIVEN an input of two comment lines and a blank line, no token
/// WHEN each build of the lexer reads it
/// THEN it prints `1:1 Eof` only, as `sisu build --emit tokens` does
#[test]
fn selfhost_lexer_should_match_emit_tokens_on_an_input_of_only_comments() {
    compare_text(
        "selfhost_lexer_should_match_emit_tokens_on_an_input_of_only_comments",
        "// one\n\n// two\n",
    );
}

/// GIVEN the input `x`, a comment line and a blank line, so a `Newline` stays pending to the end
/// WHEN each build of the lexer reads it
/// THEN the `Newline` is printed at 1:2, before the `Eof`, as in `sisu build --emit tokens`
#[test]
fn selfhost_lexer_should_match_emit_tokens_on_a_newline_pending_at_the_end() {
    compare_text(
        "selfhost_lexer_should_match_emit_tokens_on_a_newline_pending_at_the_end",
        "x\n// comment\n\n",
    );
}

/// GIVEN a `let` with the type `Box<i64>`, whose `>` ends the line, and a `let` after it
/// WHEN each build of the lexer reads it
/// THEN a `Newline` follows the `>`, as in `sisu build --emit tokens`
#[test]
fn selfhost_lexer_should_match_emit_tokens_on_a_greater_than_that_closes_a_type() {
    compare_text(
        "selfhost_lexer_should_match_emit_tokens_on_a_greater_than_that_closes_a_type",
        "fn f() {\n    let b: Box<i64>\n    let c = 1\n}\n",
    );
}

/// GIVEN a field of the type `i64??`, whose `??` ends the line, and a field after it
/// WHEN each build of the lexer reads it
/// THEN a `Newline` follows the `??`, as in `sisu build --emit tokens`
#[test]
fn selfhost_lexer_should_match_emit_tokens_on_a_question_question_that_closes_a_type() {
    compare_text(
        "selfhost_lexer_should_match_emit_tokens_on_a_question_question_that_closes_a_type",
        "class C {\n    x: i64??\n    y: i64\n}\n",
    );
}

/// GIVEN a header whose bound list breaks after `&`
/// WHEN each build of the lexer reads it
/// THEN no `Newline` follows the `&` or the name before it, as in `sisu build --emit tokens`
#[test]
fn selfhost_lexer_should_match_emit_tokens_on_a_line_break_inside_a_bound_list() {
    compare_text(
        "selfhost_lexer_should_match_emit_tokens_on_a_line_break_inside_a_bound_list",
        "fn f<T: A &\n    B\n    & C>() {}\n",
    );
}

/// GIVEN a `}` and an `else` with a `;` between them on one line
/// WHEN each build of the lexer reads it
/// THEN the `;` prints nothing, as in `sisu build --emit tokens`
#[test]
fn selfhost_lexer_should_match_emit_tokens_on_a_semicolon_before_else() {
    compare_text(
        "selfhost_lexer_should_match_emit_tokens_on_a_semicolon_before_else",
        "fn f() {\n    if a {}; else {}\n}\n",
    );
}

/// GIVEN a `}` and an `else` on the next line
/// WHEN each build of the lexer reads it
/// THEN a `Newline` follows the `}`, as in `sisu build --emit tokens`
#[test]
fn selfhost_lexer_should_match_emit_tokens_on_an_else_at_the_start_of_a_line() {
    compare_text(
        "selfhost_lexer_should_match_emit_tokens_on_an_else_at_the_start_of_a_line",
        "fn f() {\n    if a {}\n    else {}\n}\n",
    );
}

/// GIVEN a line that ends with `_`, and a line after it
/// WHEN each build of the lexer reads it
/// THEN a `Newline` follows the `_`, as in `sisu build --emit tokens`
#[test]
fn selfhost_lexer_should_match_emit_tokens_on_an_underscore_at_the_end_of_a_line() {
    compare_text(
        "selfhost_lexer_should_match_emit_tokens_on_an_underscore_at_the_end_of_a_line",
        "fn f() {\n    _\n    x\n}\n",
    );
}

/// GIVEN a line that ends with `Self`, and a line after it
/// WHEN each build of the lexer reads it
/// THEN a `Newline` follows the `Self`, as in `sisu build --emit tokens`
#[test]
fn selfhost_lexer_should_match_emit_tokens_on_self_at_the_end_of_a_line() {
    compare_text(
        "selfhost_lexer_should_match_emit_tokens_on_self_at_the_end_of_a_line",
        "fn f() {\n    Self\n    x\n}\n",
    );
}

/// GIVEN a line that ends with `?`, and a line after it
/// WHEN each build of the lexer reads it
/// THEN a `Newline` follows the `?`, as in `sisu build --emit tokens`
#[test]
fn selfhost_lexer_should_match_emit_tokens_on_a_question_mark_at_the_end_of_a_line() {
    compare_text(
        "selfhost_lexer_should_match_emit_tokens_on_a_question_mark_at_the_end_of_a_line",
        "fn f() {\n    x?\n    y\n}\n",
    );
}

/// The source of a program that prints `word` and nothing else.
fn printing(word: &str) -> String {
    format!("fn main() {{\n    print(\"{word}\")\n}}\n")
}

/// The standard output of `exe` on an empty input, which must exit 0.
fn output_of(exe: &Path) -> String {
    let out = support::Invocation::default().run(exe, false);
    assert_eq!(out.status.code(), Some(0), "{}", exe.display());
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// GIVEN a scratch copy of a program and an empty scratch cache
/// WHEN the build runs twice in the cache
/// THEN the second call returns the same paths and leaves each executable's modification time
/// unchanged
#[test]
fn selfhost_build_should_reuse_its_cache() {
    let name = "selfhost_build_should_reuse_its_cache";
    let dir = fixture(name, &[("lexer.sisu", &printing("first"))]);
    let (source, cache) = (dir.join("lexer.sisu"), dir.join("cache"));
    let modified = |builds: &selfhost::Builds| {
        [&builds.o0, &builds.o2, &builds.rc_checks].map(|exe| {
            fs::metadata(exe)
                .and_then(|metadata| metadata.modified())
                .expect("a build should have a modification time")
        })
    };
    let first = selfhost::build(&source, &cache);
    let times = modified(&first);
    let second = selfhost::build(&source, &cache);
    assert_eq!(first, second);
    assert_eq!(times, modified(&second), "the second call built again");
}

/// GIVEN a scratch copy of a program that prints `first`, built into an empty scratch cache
/// WHEN the copy changes to print `second` and the build runs again in the cache
/// THEN the new build prints `second`
#[test]
fn selfhost_build_should_rebuild_when_its_source_changes() {
    let name = "selfhost_build_should_rebuild_when_its_source_changes";
    let dir = fixture(name, &[("lexer.sisu", &printing("first"))]);
    let (source, cache) = (dir.join("lexer.sisu"), dir.join("cache"));
    assert_eq!(output_of(&selfhost::build(&source, &cache).o2), "first\n");
    fs::write(&source, printing("second")).expect("the scratch source should be writable");
    assert_eq!(output_of(&selfhost::build(&source, &cache).o2), "second\n");
}
