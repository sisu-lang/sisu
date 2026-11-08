//! End to end: `--codegen-units` splits a program into codegen units that are built on several
//! threads and linked, and a split build runs like one module. The tests read the raw IR of each
//! unit to see where a function is, and run both builds of a program to compare them.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[expect(dead_code, reason = "this binary uses only part of the shared helpers")]
mod scratch_path;
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

/// Whether the body of the function `name` in `unit` is an `available_externally` copy.
/// `None` if `unit` has no body of it.
fn copy_of(unit: &str, name: &str) -> Option<bool> {
    let text = support::function_text(unit, name)?;
    let header = text.lines().next().expect("a function has a header line");
    Some(header.contains("available_externally"))
}

/// Whether `unit` defines the function `name` itself, not as a copy.
fn defines(unit: &str, name: &str) -> bool {
    copy_of(unit, name) == Some(false)
}

/// Whether `unit` has an `available_externally` copy of the function `name`.
fn copies(unit: &str, name: &str) -> bool {
    copy_of(unit, name) == Some(true)
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

/// The text after the pipeline of a unit of `--emit ir`.
fn after_pipeline(unit: &str) -> &str {
    unit.split_once("\n; after ")
        .expect("a unit of `--emit ir` has an `; after` block")
        .1
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
/// runs `call`, which calls `second` twice, since a function with one call site would go to the
/// unit of its caller. So `--codegen-units 2` puts `main` in unit 0 and `second` in unit 1.
fn with_larger_main(second: &str, call: &str) -> String {
    let prints = "    print(0)\n".repeat(10);
    format!("{second}\n\nfn main() {{\n{prints}    {call}\n}}\n")
}

/// GIVEN a program of nine functions and methods, with a class, a generic class and an `any`
/// value of an interface that inherits another, and a small function `twice` that `main` calls
/// WHEN it is built with `--codegen-units 4` and with `--codegen-units 1`, plain and with `-O2`
/// THEN the split build reports 4 codegen units, and the unit that defines `twice` is not the unit
/// of `main`, which calls it and, at `-O2`, has an `available_externally` copy of it
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

        // `main` calls `twice` from another unit, which declares it, or at `-O2` has a copy.
        let check = |what: &str, all: &[&str], copied: bool| {
            let (twice, main) = (home(all, "twice"), home(all, "main"));
            assert_ne!(
                twice, main,
                "{row}, {what}: `twice` and `main` share a unit"
            );
            let has = if copied {
                copies(all[main], "twice")
            } else {
                declares(all[main], "twice")
            };
            assert!(
                has && calls(all[main], "twice"),
                "{row}, {what}: the unit of `main` should hold `twice` (copy: {copied}) and call it"
            );
        };
        let raw = emit(
            &dir,
            "codegen_units",
            &["--emit", "ir-raw", "--codegen-units", "4"],
        );
        check("raw IR", &units(&raw), false);
        if !level.is_empty() {
            let ir = emit(
                &dir,
                "codegen_units",
                &["--emit", "ir", "-O2", "--codegen-units", "4"],
            );
            let before: Vec<&str> = units(&ir).into_iter().map(before_pipeline).collect();
            check("IR before the pipeline", &before, true);
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
/// AND with `-O2`, the unit of `even` has an `available_externally` body of `odd`, and the unit of
/// `odd` one of `even`
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
    let ir = emit(
        &dir,
        "mutual",
        &["--emit", "ir", "-O2", "--codegen-units", "3"],
    );
    let before: Vec<&str> = units(&ir).into_iter().map(before_pipeline).collect();
    // `even` and `odd` are at `SMALL`, the largest size that a unit copies. A lower `SMALL` must
    // make them smaller, or this check must go.
    assert!(copies(before[home(&before, "even")], "odd"), "{ir}");
    assert!(copies(before[home(&before, "odd")], "even"), "{ir}");
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

/// GIVEN a program whose hot loop calls a small function and prints a result that the function
/// computes
/// WHEN it is built with -O2 --codegen-units 2
/// THEN the raw IR puts the loop and the function in different units, with a call between them
/// AND the optimized IR of the loop's unit has no call to that function
/// AND the program prints the same result as with --codegen-units 1
/// AND the test fails when the copies of C6 are off
#[test]
fn small_function_of_another_unit_should_be_inlined() {
    let source = "\
fn step(i: i64) -> i64 {
    i * 3 + 1
}

fn main() {
    let mut sum = 0
    for i in 0..1000000 {
        sum += step(i) % 7 + step(0) % 1
    }
    print(sum)
}
";
    let dir = fixture(
        "small_function_of_another_unit_should_be_inlined",
        &[("hot_loop.sisu", source)],
    );
    let ir = emit(
        &dir,
        "hot_loop",
        &["--emit", "ir", "-O2", "--codegen-units", "2"],
    );
    let all = units(&ir);
    let before: Vec<&str> = all.iter().copied().map(before_pipeline).collect();
    let (step, main) = (home(&before, "step"), home(&before, "main"));
    assert_ne!(step, main, "`step` and `main` share a unit:\n{ir}");
    assert!(
        calls(before[main], "step"),
        "the loop should call `step`:\n{ir}"
    );
    assert!(
        !calls(after_pipeline(all[main]), "step"),
        "the unit of `main` should inline `step`:\n{ir}"
    );
    let outputs = ["1", "2"].map(|count| {
        let (exe, _) = build(&dir, "hot_loop", count, &["-O2", "--codegen-units", count]);
        run(&exe)
    });
    assert_eq!(outputs[0].status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&outputs[0].stdout), "2999998\n");
    assert_eq!(outputs[0].stdout, outputs[1].stdout);
}

/// The sieve of `bench/sieve`, with `main` that calls it once with a constant, then `extra`, and
/// eight large functions that nothing calls, so that one unit's share holds `sieve` and `main`.
fn sieve_with_pads(extra: &str) -> String {
    let mut source = format!(
        "\
fn sieve(n: i64) -> [bool] {{
    let mut is_prime = [true; n + 1]
    is_prime[0] = false
    is_prime[1] = false
    for i in 2..n + 1 {{
        if is_prime[i] {{
            let mut j = i * i
            while j <= n {{
                is_prime[j] = false
                j += i
            }}
        }}
    }}
    is_prime
}}

fn main() {{
    let mut count = 0
    for p in sieve(1000) {{
        if p {{ count += 1 }}
    }}
    print(count{extra})
}}
"
    );
    for pad in 0..8 {
        let sum = (0..80)
            .map(|k| format!("n + {k}"))
            .collect::<Vec<_>>()
            .join(" + ");
        let _ = writeln!(source, "fn pad{pad}(n: i64) -> i64 {{ {sum} }}");
    }
    source
}

/// GIVEN a sieve-shaped program: a large function `sieve` that `main` calls once with a constant,
/// next to eight large functions that nothing calls, so that one unit's share holds `sieve` and
/// `main`
/// WHEN `--emit ir -O2 --codegen-units 8` prints its units
/// THEN `sieve` and `main` are defined in the same unit
/// AND the optimized IR of that unit has no call to `sieve`
/// AND the executable prints the same primes as with `--codegen-units 1`
#[test]
fn hot_function_with_one_caller_should_share_its_callers_unit() {
    let source = sieve_with_pads("");
    let dir = fixture(
        "hot_function_with_one_caller_should_share_its_callers_unit",
        &[("sieve.sisu", &source)],
    );
    let ir = emit(
        &dir,
        "sieve",
        &["--emit", "ir", "-O2", "--codegen-units", "8"],
    );
    let all = units(&ir);
    let before: Vec<&str> = all.iter().copied().map(before_pipeline).collect();
    assert_eq!(all.len(), 8, "{ir}");
    let (sieve, main) = (home(&before, "sieve"), home(&before, "main"));
    assert_eq!(
        sieve, main,
        "`sieve` and `main` are in different units:\n{ir}"
    );
    assert!(
        !calls(after_pipeline(all[main]), "sieve"),
        "the unit of `main` should inline `sieve`:\n{ir}"
    );
    let outputs = ["1", "8"].map(|count| {
        let (exe, _) = build(&dir, "sieve", count, &["-O2", "--codegen-units", count]);
        run(&exe)
    });
    assert_eq!(String::from_utf8_lossy(&outputs[0].stdout), "168\n");
    assert_eq!(outputs[0].stdout, outputs[1].stdout);
}

/// GIVEN a program in 8 units where `sieve` is called only from the unit of `main`, and `twice`
/// is called from `main` in another unit
/// WHEN `--emit ir --codegen-units 8` prints its units
/// THEN `twice` is defined `hidden`, with external linkage, in its unit
/// AND `sieve` and the pads are defined `internal`
#[test]
fn function_should_be_external_only_when_another_unit_calls_it() {
    let mut source = sieve_with_pads(" + twice(1) + twice(2)");
    source.push_str("fn twice(n: i64) -> i64 {\n    let mut s = n\n");
    for k in 0..40 {
        let _ = writeln!(source, "    s += n * {k}");
    }
    source.push_str("    s\n}\n");
    let dir = fixture(
        "function_should_be_external_only_when_another_unit_calls_it",
        &[("sieve.sisu", &source)],
    );
    let ir = emit(&dir, "sieve", &["--emit", "ir", "--codegen-units", "8"]);
    let all = units(&ir);
    let before: Vec<&str> = all.iter().copied().map(before_pipeline).collect();
    let header = |name: &str| {
        let text = support::function_text(before[home(&before, name)], name)
            .expect("the owner has a body");
        text.lines().next().expect("a header line").to_owned()
    };
    assert_ne!(
        home(&before, "twice"),
        home(&before, "main"),
        "the premise: `twice` is in another unit than `main`:\n{ir}"
    );
    assert!(header("twice").contains("hidden"), "{}", header("twice"));
    assert!(!header("twice").contains("internal"), "{}", header("twice"));
    for name in ["sieve", "pad0", "pad7"] {
        assert!(header(name).starts_with("internal"), "{}", header(name));
    }
}

/// Four large functions that nothing calls, so that the greedy rule spreads a small program over
/// the units.
fn pads() -> String {
    let mut text = String::new();
    for pad in 0..4 {
        let sum: Vec<String> = (0..60).map(|k| format!("n + {k}")).collect();
        let _ = writeln!(text, "fn pad{pad}(n: i64) -> i64 {{ {} }}", sum.join(" + "));
    }
    text
}

/// Builds `source` with `--codegen-units 1` and with 4 and runs both. Before that it prints the raw
/// IR of the 4 units and checks that the unit which defines `method` is not a unit that calls it:
/// a helper of another unit calls it, and no `Call` of the mir names it.
fn written_method_across_units(test: &str, source: &str, method: &str, want: &str) {
    let dir = fixture(test, &[("helper.sisu", source)]);
    let ir = emit(
        &dir,
        "helper",
        &["--emit", "ir-raw", "--codegen-units", "4"],
    );
    let all = units(&ir);
    assert_eq!(all.len(), 4, "{ir}");
    let home = home(&all, method);
    let callers: Vec<usize> = (0..all.len())
        .filter(|&at| calls(all[at], method))
        .collect();
    assert!(
        !callers.is_empty() && !callers.contains(&home),
        "`{method}` is in unit {home} and unit(s) {callers:?} call it:\n{ir}"
    );
    let outputs = ["1", "4"].map(|count| {
        let (exe, _) = build(&dir, "helper", count, &["--codegen-units", count]);
        run(&exe)
    });
    assert_eq!(
        outputs[0].status.code(),
        Some(0),
        "one unit: {:?}",
        outputs[0]
    );
    assert_eq!(
        String::from_utf8_lossy(&outputs[0].stdout),
        want,
        "one unit"
    );
    assert_eq!(
        outputs[1].status, outputs[0].status,
        "4 units: {:?}",
        outputs[1]
    );
    assert_eq!(outputs[1].stdout, outputs[0].stdout, "4 units");
}

/// GIVEN a class with a written `Eq` and no `any` table, which an array comparison uses through a
/// helper
/// WHEN the program is built with `--codegen-units 4` and with `--codegen-units 1`
/// THEN the unit that defines `eq` is not a unit that calls it
/// AND both executables print the same lines
#[test]
fn written_eq_called_by_a_helper_in_another_unit_should_link_and_run() {
    let source = format!(
        "class P: Eq {{
    v: i64
    fn eq(self, other: Self) -> bool {{ self.v == other.v }}
}}

fn check(a: P, b: P) -> bool {{
    [a] == [b]
}}

fn main() {{
    print(check(P(v: 1), P(v: 1)))
    print(check(P(v: 1), P(v: 2)))
}}
{}",
        pads()
    );
    written_method_across_units(
        "written_eq_called_by_a_helper_in_another_unit_should_link_and_run",
        &source,
        "P.eq",
        "true\nfalse\n",
    );
}

/// GIVEN a class with a written `Display` and no `any` table, which `print` uses through a helper
/// WHEN the program is built with `--codegen-units 4` and with `--codegen-units 1`
/// THEN the unit that defines `to_string` is not a unit that calls it
/// AND both executables print the same lines
#[test]
fn written_display_called_by_a_helper_in_another_unit_should_link_and_run() {
    let source = format!(
        "class P: Display {{
    v: i64
    fn to_string(self) -> string {{ \"P{{self.v}}\" }}
}}

fn show(p: P) {{
    print(p)
}}

fn main() {{
    show(P(v: 1))
    show(P(v: 2))
}}
{}",
        pads()
    );
    written_method_across_units(
        "written_display_called_by_a_helper_in_another_unit_should_link_and_run",
        &source,
        "P.to_string",
        "P1\nP2\n",
    );
}

/// GIVEN the case program built at `-O2` with `--codegen-units 4`
/// WHEN `sisu` builds it three times, each in its own process: with `SISU_UNIT_ORDER=forward`,
/// with `SISU_UNIT_ORDER=reverse`, and with no such variable, each to the same file name in its
/// own directory
/// THEN the three executables are the same bytes
#[test]
fn executable_should_not_depend_on_unit_order() {
    let dir = fixture(
        "executable_should_not_depend_on_unit_order",
        &[("codegen_units.sisu", CASE)],
    );
    // One file name in three directories: macOS's linker signs each executable under its name.
    let [forward, reverse, threads] = [Some("forward"), Some("reverse"), None].map(|order| {
        let build = dir.join(format!("order-{}", order.unwrap_or("threads")));
        fs::create_dir_all(&build).expect("creates the build directory");
        let exe = build.join("codegen_units");
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
    let source = with_larger_main(
        "fn boom(n: i64) {\n    panic(\"bad {n}\")\n}",
        "boom(7)\n    boom(8)",
    );
    let dir = fixture(
        "panic_in_another_unit_should_name_its_place",
        &[("boom.sisu", &source)],
    );
    let ir = emit(&dir, "boom", &["--emit", "ir-raw", "--codegen-units", "2"]);
    let all = units(&ir);
    assert_eq!(home(&all, "boom"), 1);
    assert_eq!(home(&all, "main"), 0);
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
        "let kept = make()\n    if false {\n        let _extra = make()\n    }",
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
        "print(grow() + grow())",
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

/// A program of `copies` functions of the same size and a `main` that calls them all, for a test
/// that needs a program above the size where the compiler splits a build by itself.
fn many_functions(copies: usize) -> String {
    let functions: String = (0..copies).fold(String::new(), |mut text, at| {
        let _ = writeln!(
            text,
            "fn work{at}(n: i64) -> i64 {{\n    let a = n + {at}\n    let b = a * 2\n    \
             if b > 10 {{\n        b - 1\n    }} else {{\n        b + 1\n    }}\n}}\n"
        );
        text
    });
    let calls: String = (0..copies).fold(String::new(), |mut text, at| {
        let _ = writeln!(text, "    sum += work{at}({at})");
        text
    });
    format!("{functions}fn main() {{\n    let mut sum = 0\n{calls}    print(sum)\n}}\n")
}

/// GIVEN a program of 2000 functions, which `sisu build` splits into several codegen units
/// WHEN `--emit ir -O2` and `--emit ir-raw` print it without `--codegen-units`
/// THEN the output has no `; codegen unit` line and equals the output with `--codegen-units 1`
/// AND the output of `--emit ir -O2` has one `; before default<O2>` line
///
/// 2000 copies are about twice `2 * UNIT_SIZE`, so a size rule applied to the IR output would
/// split them too.
#[test]
fn emit_ir_should_print_one_module_without_the_flag() {
    let source = many_functions(2000);
    let dir = fixture(
        "emit_ir_should_print_one_module_without_the_flag",
        &[("many.sisu", &source)],
    );
    let (_, stderr) = build(&dir, "many", "auto", &["--timings"]);
    let units: usize = (stderr.lines())
        .find_map(|line| line.strip_prefix("timings: codegen units: "))
        .and_then(|count| count.parse().ok())
        .unwrap_or_else(|| panic!("no codegen units line: {stderr}"));
    assert!(units > 1, "the build should split the program: {stderr}");
    for flags in [&["--emit", "ir", "-O2"][..], &["--emit", "ir-raw"]] {
        let plain = emit(&dir, "many", flags);
        let one: Vec<&str> = flags
            .iter()
            .chain(&["--codegen-units", "1"])
            .copied()
            .collect();
        assert!(plain == emit(&dir, "many", &one), "{flags:?}");
        assert!(!plain.contains("; codegen unit"), "{flags:?}");
        if flags.contains(&"-O2") {
            let headers = plain.lines().filter(|l| *l == "; before default<O2>");
            assert_eq!(headers.count(), 1);
        }
    }
}

/// GIVEN a program of 3 functions, where `main` calls each of the other two twice
/// WHEN it is built with `--codegen-units 64 --timings`
/// THEN it runs right and `sisu` reports 3 codegen units
#[test]
fn a_count_above_the_functions_should_build_and_run() {
    let source = "fn one() -> i64 {\n    1\n}\n\nfn two() -> i64 {\n    2\n}\n\nfn main() {\n    print(one() + one())\n    print(two() + two())\n    print(3)\n}\n";
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
    assert_eq!(String::from_utf8_lossy(&out.stdout), "2\n4\n3\n");
}

/// GIVEN a program of 3 functions, where `main` calls each of the other two twice
/// AND a `PATH` whose `cc` writes a line to stderr and exits 1, and an empty `TMPDIR`
/// WHEN `sisu build --codegen-units 3` runs
/// THEN it exits 1 with the line of `cc`, so the link ran
/// AND `TMPDIR` holds no object file, so the link error removed all three
///
/// The test gives no `--linker`: `sisu` finds a missing linker before the compile, so the link
/// would not run.
#[test]
fn failed_link_of_a_split_build_should_remove_every_object() {
    let source = "fn one() -> i64 {\n    1\n}\n\nfn two() -> i64 {\n    2\n}\n\nfn main() {\n    print(one() + one())\n    print(two() + two())\n}\n";
    let dir = fixture(
        "failed_link_of_a_split_build_should_remove_every_object",
        &[("link.sisu", source)],
    );
    // The premise: the build has 3 units, so the failed link has 3 objects to remove.
    let ir = emit(&dir, "link", &["--emit", "ir-raw", "--codegen-units", "3"]);
    assert_eq!(units(&ir).len(), 3, "{ir}");
    let failing_cc = scratch_path::failing_cc("split_link_failure_path", "fake cc: the link fails");
    let tmp = scratch_path::empty_dir("split_link_failure_tmp");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(&dir)
        .env("PATH", &failing_cc)
        .env("TMPDIR", &tmp)
        .env_remove("COMPILER_PATH")
        .args(["build", "--codegen-units", "3"])
        .args(["link.sisu", "-o", "link-out"])
        .output()
        .expect("sisu starts");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("cc failed"), "{stderr}");
    assert!(stderr.contains("fake cc: the link fails"), "{stderr}");
    let left: Vec<_> = fs::read_dir(&tmp)
        .expect("reads TMPDIR")
        .map(|entry| entry.expect("reads an entry").file_name())
        .collect();
    assert_eq!(left, Vec::<std::ffi::OsString>::new());
}

/// GIVEN a program of 9 functions and methods
/// AND the limit `ulimit -v 524288` (512 MiB), under which `--codegen-units 1` builds it
/// WHEN `sisu build --codegen-units 64` builds it, with an empty `TMPDIR`
/// THEN it exits 0, because a thread that cannot start leaves its units to the calling thread
/// AND the executable prints the lines of the case
/// AND `TMPDIR` holds no object file
///
/// Each worker thread maps 64 MiB of stack, so the limit stops some of them from starting, but
/// only on a machine with 3 or more cores. With fewer cores, every worker starts and the test
/// proves less.
#[cfg(target_os = "linux")]
#[test]
fn split_build_should_succeed_when_threads_cannot_start() {
    let dir = fixture(
        "split_build_should_succeed_when_threads_cannot_start",
        &[("codegen_units.sisu", CASE)],
    );
    let tmp = scratch_path::empty_dir("limited_split_tmp");
    let exe = dir.join("limited");
    let out = Command::new("sh")
        .current_dir(&dir)
        .env("TMPDIR", &tmp)
        .args(["-c", "ulimit -c 0; ulimit -v 524288; exec \"$0\" \"$@\""])
        .arg(env!("CARGO_BIN_EXE_sisu"))
        .args(["build", "--codegen-units", "64", "codegen_units.sisu", "-o"])
        .arg(&exe)
        .output()
        .expect("sh starts");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let ran = run(&exe);
    assert_eq!(String::from_utf8_lossy(&ran.stdout), CASE_OUT);
    let left: Vec<_> = fs::read_dir(&tmp)
        .expect("reads TMPDIR")
        .map(|entry| entry.expect("reads an entry").file_name())
        .collect();
    assert_eq!(left, Vec::<std::ffi::OsString>::new());
}
