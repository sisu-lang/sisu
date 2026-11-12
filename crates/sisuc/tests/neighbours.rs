//! Each value of `neighbours.txt` in each of its contexts: the forms that the checker treats in
//! their own way, in each position that a join or an expected type gives them. Most of the
//! programs are wrong, so the test asks only that `sisu check` ends with its own exit status, not
//! with a crash.

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

/// Runs `crash` on each program of part `n`, one after the other, and asserts that none crashed.
fn check_part(n: usize) {
    let programs = programs();
    let crashes: Vec<String> = (programs.iter().enumerate())
        .filter(|&(i, _)| part(i) == n)
        .filter_map(|(_, (which, program, stmt))| crash(which, program, stmt))
        .collect();
    assert!(
        crashes.is_empty(),
        "part {n}: `sisu check` crashed on {} of {} programs:\n{}",
        crashes.len(),
        programs.len().div_ceil(PARTS),
        crashes.join("\n")
    );
}

/// GIVEN each value of `[values]` in each of its contexts, in `main` and in a generic function
/// AND each value of `[failed values]` in each context, in a function whose bound failed, in a
/// program of its own
/// WHEN `sisu check` runs on each program of the part
/// THEN no run crashes: each one ends with no error or with an error.
///
/// Each part is one test, so that nextest spreads the parts over the cores.
macro_rules! parts {
    ($($name:ident => $n:expr),+ $(,)?) => {
        $(
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
    check_should_end_with_its_own_status_on_neighbouring_forms_part_00 => 0,
    check_should_end_with_its_own_status_on_neighbouring_forms_part_01 => 1,
    check_should_end_with_its_own_status_on_neighbouring_forms_part_02 => 2,
    check_should_end_with_its_own_status_on_neighbouring_forms_part_03 => 3,
    check_should_end_with_its_own_status_on_neighbouring_forms_part_04 => 4,
    check_should_end_with_its_own_status_on_neighbouring_forms_part_05 => 5,
    check_should_end_with_its_own_status_on_neighbouring_forms_part_06 => 6,
    check_should_end_with_its_own_status_on_neighbouring_forms_part_07 => 7,
    check_should_end_with_its_own_status_on_neighbouring_forms_part_08 => 8,
    check_should_end_with_its_own_status_on_neighbouring_forms_part_09 => 9,
    check_should_end_with_its_own_status_on_neighbouring_forms_part_10 => 10,
    check_should_end_with_its_own_status_on_neighbouring_forms_part_11 => 11,
    check_should_end_with_its_own_status_on_neighbouring_forms_part_12 => 12,
    check_should_end_with_its_own_status_on_neighbouring_forms_part_13 => 13,
    check_should_end_with_its_own_status_on_neighbouring_forms_part_14 => 14,
    check_should_end_with_its_own_status_on_neighbouring_forms_part_15 => 15,
}

/// GIVEN the programs of `programs()` and the split of `part`
/// WHEN the indexes of each part are collected
/// THEN each program is in exactly one part, and `programs()` holds each value in each context.
#[test]
fn parts_should_hold_each_program_once() {
    let total = programs().len();
    let mut seen: Vec<usize> = (0..PARTS)
        .flat_map(|n| (0..total).filter(move |&i| part(i) == n))
        .collect();
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
