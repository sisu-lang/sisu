//! One trial per example of each language reference page (`docs/reference/*.md`), named
//! `reference::<page>::<line>` for the example whose `sisu` fence is on that Markdown line.
//! One more trial per page, `reference::<page>::grammar`, fails with every fault in the page's
//! Markdown. A failure names the page, the Markdown line, and what the page expects against what
//! `sisu` gives. Each example runs in its own directory, `reference/<page>/<line>`, under the case
//! directories. `reference_check` reads the pages and checks the examples.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use libtest_mimic::{Arguments, Failed, Trial};

mod reference_check;
#[expect(dead_code, reason = "this binary uses only part of the shared helpers")]
mod support;

use reference_check::markers::{markers_in, unknown_ids};
use reference_check::rules::{is_index, known_ids, rule_faults};
use reference_check::{Example, Fault, Page, check, read_page};

/// The repository root, as seen from the crate directory that tests run in.
const REPO: &str = "../..";

/// The pages, as seen from the crate directory that tests run in.
const PAGES: &str = "../../docs/reference";

fn main() {
    let args = Arguments::from_args();
    let mut pages: Vec<_> = fs::read_dir(PAGES)
        .expect("docs/reference should be readable")
        .map(|entry| entry.expect("a directory entry should be readable").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    pages.sort();
    assert!(
        !pages.is_empty(),
        "docs/reference should hold at least one page"
    );
    let pages: Vec<(String, Page)> = pages
        .iter()
        .map(|path| {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .expect("a page name should be UTF-8")
                .to_string();
            let text = fs::read_to_string(path).expect("a page should be UTF-8 text");
            (stem, read_page(&text))
        })
        .collect();
    let known = known_ids(&pages);
    let mut trials = Vec::new();
    for (stem, page) in pages {
        let page = Arc::new(page);
        let file = format!("docs/reference/{stem}.md");
        let faults = fault_lines(&file, &page.faults);
        trials.push(Trial::test(
            format!("reference::{stem}::grammar"),
            move || fail_on(&faults),
        ));
        // A page with a `{#` item gets a rules trial. The index never joins, but the trial
        // reports its IDs. The walk runs in the closure, not in `main`: nextest starts this
        // binary for each trial.
        if !page.id_lines.is_empty() {
            let (stem, page, file) = (stem.clone(), Arc::clone(&page), file.clone());
            trials.push(Trial::test(
                format!("reference::{stem}::rules"),
                move || {
                    // The index needs no marker, so the walk is for the other pages.
                    let markers = if is_index(&stem) {
                        Vec::new()
                    } else {
                        markers_in(Path::new(REPO)).map_err(Failed::from)?.0
                    };
                    let faults = rule_faults(&stem, &page, &markers);
                    fail_on(&fault_lines(&file, &faults))
                },
            ));
        }
        for index in 0..page.examples.len() {
            let example_line = page.examples[index].line;
            let name = format!("reference::{stem}::{example_line}");
            let case = format!("reference/{stem}/{example_line}");
            let (file, page) = (file.clone(), Arc::clone(&page));
            trials.push(Trial::test(name, move || {
                let example = &page.examples[index];
                let dir = support::case_dir(Path::new(&case));
                check(example, &dir)
                    .map_err(|message| Failed::from(located(&file, example, &message)))
            }));
        }
    }
    // The walk runs in the closure, not in `main`: nextest starts this binary for each trial.
    trials.push(Trial::test("reference::markers", move || {
        let (markers, mut faults) = markers_in(Path::new(REPO)).map_err(Failed::from)?;
        faults.extend(unknown_ids(&markers, &known));
        faults.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
        let lines: Vec<String> = faults
            .iter()
            .map(|fault| format!("{}:{}: {}", fault.file, fault.line, fault.message))
            .collect();
        fail_on(&lines)
    }));
    libtest_mimic::run(&args, trials).exit()
}

/// Each of `faults` as `file:line: message`.
fn fault_lines(file: &str, faults: &[Fault]) -> Vec<String> {
    faults
        .iter()
        .map(|fault| format!("{file}:{}: {}", fault.line, fault.message))
        .collect()
}

/// Fails with every line of `faults`, or passes when there is none.
fn fail_on(faults: &[String]) -> Result<(), Failed> {
    if faults.is_empty() {
        Ok(())
    } else {
        Err(Failed::from(faults.join("\n")))
    }
}

/// `message` from `check`, led by `file` and the Markdown line. A message that starts `line N: `
/// names line N of the program, which is Markdown line N after the fence; any other message names
/// the fence.
fn located(file: &str, example: &Example, message: &str) -> String {
    let (line, message) = message
        .strip_prefix("line ")
        .and_then(|rest| rest.split_once(": "))
        .and_then(|(number, rest)| Some((example.line + number.parse::<usize>().ok()?, rest)))
        .unwrap_or((example.line, message));
    format!("{file}:{line}: {message}")
}
