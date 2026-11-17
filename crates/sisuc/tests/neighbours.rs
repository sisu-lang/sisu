//! Each value of `neighbours.txt` in each of its contexts: the forms that the checker treats in
//! their own way, in each position that a join or an expected type gives them. Most of the
//! programs are wrong. So the test asks that `sisu check` ends with its own exit status, not with
//! a crash. It also asks that each safe fix keeps the contract of `sisuc::safe_fix_breaches`.

use std::path::Path;
use std::process::Command;

/// The sections of `neighbours.txt`: the template, with its two `{stmt}` places, and the function
/// whose bound failed, with its one. Then the values, the failed values and the contexts, one for
/// each line that is not empty and not a comment.
struct Forms {
    template: String,
    failed: String,
    values: Vec<String>,
    failed_values: Vec<String>,
    contexts: Vec<String>,
}

fn forms() -> Forms {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/neighbours.txt");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));
    let mut forms = Forms {
        template: String::new(),
        failed: String::new(),
        values: Vec::new(),
        failed_values: Vec::new(),
        contexts: Vec::new(),
    };
    let mut section = "";
    for line in text.lines() {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']'))
            && ["template", "failed", "values", "failed values", "contexts"].contains(&name)
        {
            section = name;
            continue;
        }
        match section {
            "template" => {
                forms.template.push_str(line);
                forms.template.push('\n');
            }
            "failed" => {
                forms.failed.push_str(line);
                forms.failed.push('\n');
            }
            _ if line.trim().is_empty() || line.starts_with('#') => {}
            "values" => forms.values.push(line.to_string()),
            "failed values" => forms.failed_values.push(line.to_string()),
            "contexts" => forms.contexts.push(line.to_string()),
            _ => {}
        }
    }
    assert_eq!(
        forms.template.matches("{stmt}").count(),
        2,
        "the template should have a `{{stmt}}` in a generic function and in `main`"
    );
    assert_eq!(
        forms.failed.matches("{stmt}").count(),
        1,
        "the function whose bound failed should have one `{{stmt}}`"
    );
    assert!(
        !forms.failed_values.is_empty() && forms.failed_values.iter().all(|v| v.contains('u')),
        "each value of `[failed values]` should read `u`, and there should be one"
    );
    assert!(
        forms.contexts.iter().all(|c| c.contains('$')),
        "each context should have a `$` for the value"
    );
    forms
}

/// The program (`which`), the statement and the output of `sisu check` when it crashed on that
/// statement.
fn crash(which: &str, program: &str, stmt: &str) -> Option<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .args(["check", "-e", program])
        .output()
        .expect("sisu should start");
    // 0: no error; 1: an error. A panic aborts, and a signal has no exit code.
    (!matches!(out.status.code(), Some(0 | 1))).then(|| {
        format!(
            "{which}: {stmt}\n  {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )
    })
}

/// The number of tests that share the programs. Each part is one test, so that nextest spreads
/// them over the cores.
const PARTS: usize = 16;

/// Each program, as the template or the failed function, the program and its statement: each value
/// of `[values]` in each context, then each value of `[failed values]` in each context.
fn programs() -> Vec<(&'static str, String, String)> {
    let forms = forms();
    let statements = |values: &[String]| -> Vec<String> {
        (forms.contexts.iter())
            .flat_map(|c| values.iter().map(move |v| c.replace('$', v)))
            .collect()
    };
    let bare = forms.template.replace("{stmt}", "");
    let mut programs = Vec::new();
    for stmt in statements(&forms.values) {
        programs.push(("template", forms.template.replace("{stmt}", &stmt), stmt));
    }
    for stmt in statements(&forms.failed_values) {
        let program = bare.clone() + &forms.failed.replace("{stmt}", &stmt);
        programs.push(("failed", program, stmt));
    }
    programs
}

/// The part of the program at `index`. Not slices: the contexts of `neighbours.txt` are in groups,
/// so slices would give the parts unequal costs.
fn part(index: usize) -> usize {
    index % PARTS
}

/// The indexes of the programs of part `n`, out of `total` programs.
fn part_indexes(n: usize, total: usize) -> impl Iterator<Item = usize> {
    (0..total).filter(move |&i| part(i) == n)
}

/// Runs `crash` on each program of part `n`, one after the other, and asserts that none crashed.
/// Then asserts that no safe fix of those programs breaks its contract.
fn check_part(n: usize) {
    let programs = programs();
    let indexes: Vec<usize> = part_indexes(n, programs.len()).collect();
    let crashes: Vec<String> = (indexes.iter())
        .filter_map(|&i| {
            let (which, program, stmt) = &programs[i];
            crash(which, program, stmt)
        })
        .collect();
    assert!(
        crashes.is_empty(),
        "part {n}: `sisu check` crashed on {} of {} programs:\n{}",
        crashes.len(),
        indexes.len(),
        crashes.join("\n")
    );
    let breaches: Vec<String> = std::thread::Builder::new()
        .stack_size(sisuc::STACK_SIZE)
        .spawn(move || {
            (indexes.iter())
                .flat_map(|&i| {
                    let (which, program, stmt) = &programs[i];
                    let breaches = sisuc::safe_fix_breaches(program);
                    breaches
                        .into_iter()
                        .map(move |b| format!("{which}: {stmt}\n  {b}"))
                })
                .collect()
        })
        .expect("the thread should start")
        .join()
        .expect("the check of the safe fixes should not panic");
    assert!(
        breaches.is_empty(),
        "part {n}: {} safe fixes break their contract:\n{}",
        breaches.len(),
        breaches.join("\n")
    );
}

/// Each part is one test, so that nextest spreads the parts over the cores.
macro_rules! parts {
    ($($name:ident => $n:expr),+ $(,)?) => {
        $(
            /// GIVEN each value of `[values]` in each of its contexts, in `main` and in a generic
            /// function
            /// AND each value of `[failed values]` in each context, in a function whose bound
            /// failed, in a program of its own
            /// WHEN `sisu check` runs on each program of the part
            /// THEN no run crashes: each one ends with no error or with an error
            /// AND each safe fix, applied alone, removes only the diagnostics that carry it and
            /// adds no syntax error.
            #[test]
            fn $name() {
                check_part($n);
            }
        )+

        /// The `n` of each part test.
        const TESTED_PARTS: [usize; PARTS] = [$($n),+];

        // Each of `0..PARTS` is in `TESTED_PARTS` exactly once.
        const _: () = {
            let mut n = 0;
            while n < PARTS {
                let mut count = 0;
                let mut i = 0;
                while i < PARTS {
                    if TESTED_PARTS[i] == n {
                        count += 1;
                    }
                    i += 1;
                }
                assert!(count == 1, "each part should have one test");
                n += 1;
            }
        };
    };
}

parts! {
    neighbouring_forms_should_check_with_no_crash_and_no_breaking_safe_fix_part_00 => 0,
    neighbouring_forms_should_check_with_no_crash_and_no_breaking_safe_fix_part_01 => 1,
    neighbouring_forms_should_check_with_no_crash_and_no_breaking_safe_fix_part_02 => 2,
    neighbouring_forms_should_check_with_no_crash_and_no_breaking_safe_fix_part_03 => 3,
    neighbouring_forms_should_check_with_no_crash_and_no_breaking_safe_fix_part_04 => 4,
    neighbouring_forms_should_check_with_no_crash_and_no_breaking_safe_fix_part_05 => 5,
    neighbouring_forms_should_check_with_no_crash_and_no_breaking_safe_fix_part_06 => 6,
    neighbouring_forms_should_check_with_no_crash_and_no_breaking_safe_fix_part_07 => 7,
    neighbouring_forms_should_check_with_no_crash_and_no_breaking_safe_fix_part_08 => 8,
    neighbouring_forms_should_check_with_no_crash_and_no_breaking_safe_fix_part_09 => 9,
    neighbouring_forms_should_check_with_no_crash_and_no_breaking_safe_fix_part_10 => 10,
    neighbouring_forms_should_check_with_no_crash_and_no_breaking_safe_fix_part_11 => 11,
    neighbouring_forms_should_check_with_no_crash_and_no_breaking_safe_fix_part_12 => 12,
    neighbouring_forms_should_check_with_no_crash_and_no_breaking_safe_fix_part_13 => 13,
    neighbouring_forms_should_check_with_no_crash_and_no_breaking_safe_fix_part_14 => 14,
    neighbouring_forms_should_check_with_no_crash_and_no_breaking_safe_fix_part_15 => 15,
}

/// GIVEN the template of `neighbours.txt` with each `{stmt}` empty
/// WHEN `sisu check` runs on it
/// THEN it ends with no error, so each program meets its statement and no error of the template.
#[test]
fn template_should_check_with_no_error() {
    let program = forms().template.replace("{stmt}", "");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .args(["check", "-e", &program])
        .output()
        .expect("sisu should start");
    assert!(
        out.status.success(),
        "the template should check with no error:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// GIVEN the programs of `programs()` and the split of `part`
/// WHEN the indexes of each part are collected
/// THEN each program is in exactly one part, and `programs()` holds each value in each context.
#[test]
fn parts_should_hold_each_program_once() {
    let total = programs().len();
    let mut seen: Vec<usize> = (0..PARTS).flat_map(|n| part_indexes(n, total)).collect();
    seen.sort_unstable();
    assert_eq!(
        seen,
        (0..total).collect::<Vec<_>>(),
        "each index should be in one part"
    );
    let forms = forms();
    assert_eq!(
        total,
        (forms.values.len() + forms.failed_values.len()) * forms.contexts.len(),
        "there should be one program for each value in each context"
    );
}

/// GIVEN the programs of `programs()` and a new, empty directory in `NEIGHBOURS_DUMP`
/// WHEN the test runs
/// THEN each program is in `<index>.sisu` there, the index padded to 5 digits, so that
/// `scripts/diag_diff.py` can compare two builds on them.
#[test]
#[ignore = "a dump for diag_diff"]
fn programs_should_dump_when_asked() {
    let dir = std::env::var_os("NEIGHBOURS_DUMP").expect("NEIGHBOURS_DUMP should name a directory");
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).expect("the dump directory should be made");
    let empty = std::fs::read_dir(dir)
        .expect("the dump directory should be read")
        .next()
        .is_none();
    assert!(empty, "the dump directory should be new and empty");
    for (index, (_, program, _)) in programs().iter().enumerate() {
        std::fs::write(dir.join(format!("{index:05}.sisu")), program)
            .expect("the program should be written");
    }
}
