//! Each value of `neighbours.txt` in each of its contexts: the forms that the checker treats in
//! their own way, in each position that a join or an expected type gives them. Most of the
//! programs are wrong, so the test asks only that `sisu check` ends with its own exit status, not
//! with a crash.

use std::path::Path;
use std::process::Command;
use std::thread;

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

/// GIVEN each value of `[values]` in each of its contexts, in `main` and in a generic function
/// AND each value of `[failed values]` in each context, in a function whose bound failed, in a
/// program of its own
/// WHEN `sisu check` runs on each program
/// THEN no run crashes: each one ends with no error or with an error.
#[test]
fn check_should_end_with_its_own_status_on_each_neighbouring_form() {
    let forms = forms();
    let statements = |values: &[String]| -> Vec<String> {
        (forms.contexts.iter())
            .flat_map(|c| values.iter().map(move |v| c.replace('$', v)))
            .collect()
    };
    let bare = forms.template.replace("{stmt}", "");
    let mut programs: Vec<(&str, String, String)> = Vec::new();
    for stmt in statements(&forms.values) {
        programs.push(("template", forms.template.replace("{stmt}", &stmt), stmt));
    }
    for stmt in statements(&forms.failed_values) {
        let program = bare.clone() + &forms.failed.replace("{stmt}", &stmt);
        programs.push(("failed", program, stmt));
    }
    let workers = thread::available_parallelism().map_or(4, usize::from);
    let crashes: Vec<String> = thread::scope(|s| {
        let handles: Vec<_> = programs
            .chunks(programs.len().div_ceil(workers))
            .map(|chunk| {
                s.spawn(move || {
                    chunk
                        .iter()
                        .filter_map(|(which, program, stmt)| crash(which, program, stmt))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("a worker should not panic"))
            .collect()
    });
    assert!(
        crashes.is_empty(),
        "`sisu check` crashed on {} of {} programs:\n{}",
        crashes.len(),
        programs.len(),
        crashes.join("\n")
    );
}
