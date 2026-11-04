//! End to end: `--codegen-units` splits a program into codegen units that are built on several
//! threads and linked, and a split build runs like one module. The tests read the raw IR of each
//! unit to see where a function is, and run both builds of a program to compare them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[expect(dead_code, reason = "this binary uses only part of the shared helpers")]
mod support;

use support::{Invocation, fixture};

/// The `tests/run` case that every test of a split build here shares.
const CASE: &str = include_str!("run/codegen_units.sisu");
const CASE_OUT: &str = include_str!("run/codegen_units.out");

/// The units of `ir`, the text that `--emit ir` or `--emit ir-raw` printed with `--codegen-units`.
/// It splits `ir` at each `; codegen unit <k> of <n>` line and checks that the headers count from
/// 0 to `n - 1`, each with the same `n`.
fn units(ir: &str) -> Vec<&str> {
    let mut starts: Vec<usize> = Vec::new();
    let mut offset = 0;
    for line in ir.split_inclusive('\n') {
        if line.starts_with("; codegen unit ") {
            starts.push(offset);
        }
        offset += line.len();
    }
    let count = starts.len();
    let parts: Vec<&str> = (starts.iter().enumerate())
        .map(|(at, &start)| &ir[start..starts.get(at + 1).copied().unwrap_or(ir.len())])
        .collect();
    for (at, part) in parts.iter().enumerate() {
        let header = format!("; codegen unit {at} of {count}\n");
        assert!(part.starts_with(&header), "unit {at} of {count}:\n{part}");
    }
    parts
}

/// Whether `unit` defines the function `name`.
fn defines(unit: &str, name: &str) -> bool {
    support::function_text(unit, name).is_some()
}

/// Whether `unit` has a `call` of the function `name`.
fn calls(unit: &str, name: &str) -> bool {
    let callee = format!("{}(", support::symbol(name));
    (unit.lines()).any(|line| line.contains(" call ") && line.contains(&callee))
}

/// Whether `unit` has the line `declare` of the function `name`.
fn declares(unit: &str, name: &str) -> bool {
    let callee = format!("{}(", support::symbol(name));
    (unit.lines()).any(|line| line.starts_with("declare ") && line.contains(&callee))
}

/// The index of the one unit of `units` that defines `name`.
fn home(units: &[&str], name: &str) -> usize {
    let homes: Vec<usize> = (0..units.len())
        .filter(|&at| defines(units[at], name))
        .collect();
    assert_eq!(homes.len(), 1, "`{name}` should have one body: {homes:?}");
    homes[0]
}

/// The text before the pipeline of a unit of `--emit ir`.
fn before_pipeline(unit: &str) -> &str {
    unit.split("\n; after ").next().expect("a split has a head")
}

/// The stdout of `sisu build <flags> <name>.sisu` run in `dir`, which must succeed.
fn emit(dir: &Path, name: &str, flags: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .arg("build")
        .args(flags)
        .arg(format!("{name}.sisu"))
        .output()
        .expect("sisu starts");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{flags:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("the IR is UTF-8")
}

/// Builds `<name>.sisu` in `dir` into `<name>-<tag>` with `flags`, and returns the executable and
/// the stderr of `sisu`.
fn build(dir: &Path, name: &str, tag: &str, flags: &[&str]) -> (PathBuf, String) {
    let exe = dir.join(format!("{name}-{tag}"));
    let stderr = support::compile(dir, name, &exe, flags);
    (exe, String::from_utf8_lossy(&stderr).into_owned())
}

/// Runs `exe` with no input.
fn run(exe: &Path) -> Output {
    Command::new(exe).output().expect("the program starts")
}

/// The source of `second` and a `main` that is larger than `second`: it prints ten lines and then
/// runs `call`. So `--codegen-units 2` puts `main` in unit 0 and `second` in unit 1.
fn with_larger_main(second: &str, call: &str) -> String {
    let prints = "    print(0)\n".repeat(10);
    format!("{second}\n\nfn main() {{\n{prints}    {call}\n}}\n")
}

/// GIVEN a program of nine functions and methods, with a class, a generic class and an `any`
/// value of an interface that inherits another, and a small function `twice` that `main` calls
/// WHEN it is built with `--codegen-units 4` and with `--codegen-units 1`, plain and with `-O2`
/// THEN the split build reports 4 codegen units, and the unit that defines `twice` is not the unit
/// of `main`, which declares and calls it
/// AND both executables print the lines of the case and exit with the same status
#[test]
fn split_build_should_run_like_one_module() {
    let dir = fixture(
        "split_build_should_run_like_one_module",
        &[("codegen_units.sisu", CASE)],
    );
    for (row, level) in [("plain", &[][..]), ("-O2", &["-O2"][..])] {
        let with = |extra: &[&'static str]| -> Vec<&'static str> {
            level.iter().chain(extra).copied().collect()
        };
        let (split, stderr) = build(
            &dir,
            "codegen_units",
            &format!("split{row}"),
            &with(&["--codegen-units", "4", "--timings"]),
        );
        assert!(
            stderr.lines().any(|l| l == "timings: codegen units: 4"),
            "{row}: {stderr}"
        );
        let (whole, _) = build(
            &dir,
            "codegen_units",
            &format!("whole{row}"),
            &with(&["--codegen-units", "1"]),
        );

        // `main` calls `twice` from another unit, which declares it.
        let check = |what: &str, all: &[&str]| {
            let (twice, main) = (home(all, "twice"), home(all, "main"));
            assert_ne!(
                twice, main,
                "{row}, {what}: `twice` and `main` share a unit"
            );
            assert!(
                declares(all[main], "twice") && calls(all[main], "twice"),
                "{row}, {what}: the unit of `main` should declare and call `twice`"
            );
        };
        let raw = emit(
            &dir,
            "codegen_units",
            &["--emit", "ir-raw", "--codegen-units", "4"],
        );
        check("raw IR", &units(&raw));
        if !level.is_empty() {
            let ir = emit(
                &dir,
                "codegen_units",
                &["--emit", "ir", "-O2", "--codegen-units", "4"],
            );
            let before: Vec<&str> = units(&ir).into_iter().map(before_pipeline).collect();
            check("IR before the pipeline", &before);
        }

        let [split, whole] = [split, whole].map(|exe| run(&exe));
        assert_eq!(
            String::from_utf8_lossy(&split.stdout),
            CASE_OUT,
            "{row}: split"
        );
        assert_eq!(
            String::from_utf8_lossy(&whole.stdout),
            CASE_OUT,
            "{row}: whole"
        );
        assert_eq!(split.status.code(), Some(0), "{row}");
        assert_eq!(split.status, whole.status, "{row}");
        assert_eq!(split.stderr, whole.stderr, "{row}");
    }
}

/// GIVEN the case program built with `--codegen-units 4`, plain, with `-O2` and with `--rc-checks`
/// WHEN each executable runs under Valgrind on Linux, on the system allocator, as the `tests/run`
/// cases do
/// THEN it prints the lines of the case and Valgrind finds no error
/// AND the `--rc-checks` build, run without Valgrind, prints the same and exits 0
///
/// A `tests/run` case builds one module, so this test is the memory proof of a split build.
#[test]
fn split_build_should_pass_the_memory_checks() {
    let dir = fixture(
        "split_build_should_pass_the_memory_checks",
        &[("codegen_units.sisu", CASE)],
    );
    let split = ["--codegen-units", "4"];
    let valgrind = [
        support::VALGRIND_O0,
        support::VALGRIND_O2,
        support::VALGRIND_RC,
    ];
    for (tag, flags) in valgrind {
        let flags: Vec<&str> = flags.iter().chain(&split).copied().collect();
        let (exe, _) = build(&dir, "codegen_units", tag, &flags);
        let out = Invocation::default().run(&exe, true);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{tag}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        // Valgrind arm64 runs `min` and `max` of a `nan` wrong; this case has none.
        assert_eq!(String::from_utf8_lossy(&out.stdout), CASE_OUT, "{tag}");
    }
    let (tag, flags) = support::RC_CHECKS;
    let flags: Vec<&str> = flags.iter().chain(&split).copied().collect();
    let (exe, _) = build(&dir, "codegen_units", tag, &flags);
    let out = run(&exe);
    assert_eq!(out.status.code(), Some(0), "{tag}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), CASE_OUT, "{tag}");
    assert_eq!(out.stderr, b"", "{tag}");
}

/// GIVEN `even` and `odd`, which call each other, and a `main` that calls both
/// WHEN the program is built with `--codegen-units 3`
/// THEN `even` and `odd` are in different units
/// AND with `-O2` the program prints what it prints with `--codegen-units 1`
#[test]
fn mutual_recursion_across_units_should_run() {
    let source = "\
fn even(n: i64) -> bool {
    if n == 0 {
        true
    } else {
        odd(n - 1)
    }
}

fn odd(n: i64) -> bool {
    if n == 0 {
        false
    } else {
        even(n - 1)
    }
}

fn main() {
    print(even(10))
    print(odd(7))
    print(even(7))
}
";
    let dir = fixture(
        "mutual_recursion_across_units_should_run",
        &[("mutual.sisu", source)],
    );
    let ir = emit(
        &dir,
        "mutual",
        &["--emit", "ir-raw", "--codegen-units", "3"],
    );
    let all = units(&ir);
    assert_eq!(all.len(), 3);
    assert_ne!(home(&all, "even"), home(&all, "odd"));
    let outputs = ["1", "3"].map(|count| {
        let (exe, _) = build(&dir, "mutual", count, &["-O2", "--codegen-units", count]);
        run(&exe)
    });
    assert_eq!(
        String::from_utf8_lossy(&outputs[1].stdout),
        "true\ntrue\nfalse\n"
    );
    assert_eq!(outputs[0].stdout, outputs[1].stdout);
    assert_eq!(outputs[0].status.code(), Some(0));
    assert_eq!(outputs[1].status.code(), Some(0));
}

/// GIVEN the case program built at `-O2` with `--codegen-units 4`
/// WHEN `sisu` builds it three times, each in its own process: with `SISU_UNIT_ORDER=forward`,
/// with `SISU_UNIT_ORDER=reverse`, and with no such variable
/// THEN the three executables are the same bytes
#[test]
fn executable_should_not_depend_on_unit_order() {
    let dir = fixture(
        "executable_should_not_depend_on_unit_order",
        &[("codegen_units.sisu", CASE)],
    );
    let [forward, reverse, threads] = [Some("forward"), Some("reverse"), None].map(|order| {
        let exe = dir.join(format!("order-{}", order.unwrap_or("threads")));
        let mut command = support::sisu(
            &dir,
            "codegen_units.sisu",
            &exe,
            &["-O2", "--codegen-units", "4"],
        );
        match order {
            Some(order) => command.env("SISU_UNIT_ORDER", order),
            None => command.env_remove("SISU_UNIT_ORDER"),
        };
        let out = command.output().expect("sisu starts");
        assert_eq!(out.status.code(), Some(0), "{order:?}: {out:?}");
        fs::read(&exe).expect("reads the executable")
    });
    assert!(forward == reverse, "forward and reverse differ");
    assert!(forward == threads, "forward and the threads differ");
}

/// GIVEN a function `boom` that panics, and a larger `main` that calls it
/// WHEN the program is built with `--codegen-units 2`, and with `--codegen-units 1`, and run
/// THEN unit 1 defines `boom` and unit 0 calls it
/// AND both runs exit with the same status and print the same panic line on stderr
#[test]
fn panic_in_another_unit_should_name_its_place() {
    let source = with_larger_main("fn boom(n: i64) {\n    panic(\"bad {n}\")\n}", "boom(7)");
    let dir = fixture(
        "panic_in_another_unit_should_name_its_place",
        &[("boom.sisu", &source)],
    );
    let ir = emit(&dir, "boom", &["--emit", "ir-raw", "--codegen-units", "2"]);
    let all = units(&ir);
    assert_eq!(home(&all, "boom"), 1);
    assert!(calls(all[0], "main") || defines(all[0], "main"));
    assert!(calls(all[0], "boom"), "unit 0 should call `boom`");
    let [split, whole] = ["2", "1"].map(|count| {
        let (exe, _) = build(&dir, "boom", count, &["--codegen-units", count]);
        run(&exe)
    });
    assert_eq!(split.status.code(), Some(101));
    assert_eq!(split.status, whole.status);
    assert_eq!(split.stdout, whole.stdout);
    assert_eq!(split.stderr, whole.stderr);
    assert_eq!(
        String::from_utf8_lossy(&split.stderr),
        "sisu: panic at boom.sisu:2:5: bad 7\n"
    );
}

/// GIVEN a function `make` that makes a cycle of one object, and a larger `main` that holds the
/// object and drops it
/// WHEN the program is built with `--rc-checks --codegen-units 2`, and with `--codegen-units 1`,
/// and run
/// THEN unit 1 defines `make` and unit 0 calls it
/// AND both runs exit 102 and print the same live-object line on stderr
#[test]
fn rc_checks_should_see_a_leak_across_units() {
    let source = with_larger_main(
        "class Node {\n    mut _next: Node?\n}\n\nfn make() -> Node {\n    let a = Node(_next: None)\n    a._next = a\n    a\n}",
        "let kept = make()",
    );
    let dir = fixture(
        "rc_checks_should_see_a_leak_across_units",
        &[("leak.sisu", &source)],
    );
    let ir = emit(
        &dir,
        "leak",
        &["--emit", "ir-raw", "--rc-checks", "--codegen-units", "2"],
    );
    let all = units(&ir);
    assert_eq!(home(&all, "make"), 1);
    assert!(calls(all[0], "make"), "unit 0 should call `make`");
    let [split, whole] = ["2", "1"].map(|count| {
        let (exe, _) = build(
            &dir,
            "leak",
            count,
            &["--rc-checks", "--codegen-units", count],
        );
        run(&exe)
    });
    assert_eq!(split.status.code(), Some(102), "{split:?}");
    assert_eq!(split.status, whole.status);
    assert_eq!(split.stdout, whole.stdout);
    assert_eq!(split.stderr, whole.stderr);
    assert_eq!(
        String::from_utf8_lossy(&split.stderr),
        "sisu: rc check: live objects at exit: 1\n"
    );
}

/// GIVEN a class that holds an array of itself, which only a function of unit 1 makes and drops
/// WHEN the raw IR is printed with `--codegen-units 2`
/// THEN unit 1 has a drop function that loads `@sisu_rc.depth`, and unit 0 has none
/// AND unit 0 defines `@sisu_rc.depth` and `@sisu_rc.deferred` as hidden globals
/// AND unit 1 declares both as external hidden globals
#[test]
fn drop_depth_state_should_have_one_home() {
    let source = with_larger_main(
        "class A {\n    _next: [A]\n}\n\nfn grow() -> i64 {\n    let a = A(_next: [])\n    let b = A(_next: [a])\n    b._next.len()\n}",
        "print(grow())",
    );
    let dir = fixture(
        "drop_depth_state_should_have_one_home",
        &[("deep.sisu", &source)],
    );
    let ir = emit(&dir, "deep", &["--emit", "ir-raw", "--codegen-units", "2"]);
    let all = units(&ir);
    assert_eq!(home(&all, "grow"), 1);
    let counts = |unit: &str| unit.contains("load i64, ptr @sisu_rc.depth");
    assert!(
        counts(all[1]),
        "unit 1 should have a counting drop function"
    );
    assert!(!counts(all[0]), "unit 0 should have none");
    for name in ["sisu_rc.depth", "sisu_rc.deferred"] {
        let has = |unit: &str, text: &str| {
            unit.lines()
                .any(|line| line.starts_with(&format!("@{name} = {text}")))
        };
        assert!(has(all[0], "hidden global"), "unit 0 defines `{name}`");
        assert!(
            has(all[1], "external hidden global"),
            "unit 1 declares `{name}`"
        );
    }
}

/// GIVEN a program
/// WHEN `--emit ir -O2` and `--emit ir-raw` print it without `--codegen-units`
/// THEN the output has no `; codegen unit` line and equals the output with `--codegen-units 1`
/// AND the output of `--emit ir -O2` has one `; before default<O2>` line
#[test]
fn emit_ir_should_print_one_module_without_the_flag() {
    let dir = fixture(
        "emit_ir_should_print_one_module_without_the_flag",
        &[("codegen_units.sisu", CASE)],
    );
    for flags in [&["--emit", "ir", "-O2"][..], &["--emit", "ir-raw"]] {
        let plain = emit(&dir, "codegen_units", flags);
        let one: Vec<&str> = flags
            .iter()
            .chain(&["--codegen-units", "1"])
            .copied()
            .collect();
        assert!(plain == emit(&dir, "codegen_units", &one), "{flags:?}");
        assert!(!plain.contains("; codegen unit"), "{flags:?}");
        if flags.contains(&"-O2") {
            let headers = plain.lines().filter(|l| *l == "; before default<O2>");
            assert_eq!(headers.count(), 1);
        }
    }
}

/// GIVEN a program of 3 functions
/// WHEN it is built with `--codegen-units 64 --timings`
/// THEN it runs right and `sisu` reports 3 codegen units
#[test]
fn a_count_above_the_functions_should_build_and_run() {
    let source = "fn one() -> i64 {\n    1\n}\n\nfn two() -> i64 {\n    2\n}\n\nfn main() {\n    print(one())\n    print(two())\n    print(3)\n}\n";
    let dir = fixture(
        "a_count_above_the_functions_should_build_and_run",
        &[("few.sisu", source)],
    );
    let (exe, stderr) = build(&dir, "few", "64", &["--codegen-units", "64", "--timings"]);
    assert!(
        stderr.lines().any(|l| l == "timings: codegen units: 3"),
        "{stderr}"
    );
    let out = run(&exe);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n2\n3\n");
}
