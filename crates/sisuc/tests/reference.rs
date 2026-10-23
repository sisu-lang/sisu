//! One trial per example of each language reference page (`docs/reference/*.md`), named
//! `reference::<page>::<line>` for the example whose `sisu` fence is on that Markdown line.
//! One more trial per page, `reference::<page>::grammar`, fails with every fault in the page's
//! Markdown. A failure names the page, the Markdown line, and what the page expects against what
//! `sisu` gives. Each example runs in its own directory, `reference/<page>/<line>`, under the case
//! directories. `reference_check` reads the pages and checks the examples.

use std::fs;
use std::path::Path;

use libtest_mimic::{Arguments, Failed, Trial};

mod reference_check;
#[expect(dead_code, reason = "this binary uses only part of the shared helpers")]
mod support;

use reference_check::{Example, check, read_page};

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
    let mut trials = Vec::new();
    for path in pages {
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("a page name should be UTF-8")
            .to_string();
        let text = fs::read_to_string(&path).expect("a page should be UTF-8 text");
        let page = read_page(&text);
        let file = format!("docs/reference/{stem}.md");
        let faults: Vec<String> = page
            .faults
            .iter()
            .map(|fault| format!("{file}:{}: {}", fault.line, fault.message))
            .collect();
        trials.push(Trial::test(
            format!("reference::{stem}::grammar"),
            move || {
                if faults.is_empty() {
                    Ok(())
                } else {
                    Err(Failed::from(faults.join("\n")))
                }
            },
        ));
        for example in page.examples {
            let name = format!("reference::{stem}::{}", example.line);
            let case = format!("reference/{stem}/{}", example.line);
            let file = file.clone();
            trials.push(Trial::test(name, move || {
                let dir = support::case_dir(Path::new(&case));
                check(&example, &dir)
                    .map_err(|message| Failed::from(located(&file, &example, &message)))
            }));
        }
    }
    libtest_mimic::run(&args, trials).exit()
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
