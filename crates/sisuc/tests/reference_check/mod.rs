//! The checker of the examples in a language reference page (`docs/reference/*.md`).
//!
//! `read_page` reads a page's fenced blocks with `pulldown-cmark` and finds its examples and the
//! faults in its Markdown. `check` writes one example to `example.sisu` and checks it as its
//! kind says. First, `sisu check --format json` must report its diagnostic markers. Then one
//! plain build runs. Last, one run must give the exit, standard output and standard error that
//! the page says.

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::Command;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use serde_json::Value;

use crate::support;

/// The examples of a page and the faults in its Markdown.
#[derive(Debug)]
pub(crate) struct Page {
    pub(crate) examples: Vec<Example>,
    pub(crate) faults: Vec<Fault>,
}

/// A fault in a page's Markdown, on its 1-based `line`.
#[derive(Debug)]
pub(crate) struct Fault {
    pub(crate) line: usize,
    pub(crate) message: String,
}

/// A `sisu` example and its companion blocks.
#[derive(Debug)]
pub(crate) struct Example {
    /// The Markdown line of the opening fence.
    pub(crate) line: usize,
    pub(crate) source: String,
    pub(crate) kind: Kind,
    pub(crate) markers: Vec<Marker>,
    pub(crate) args: Vec<String>,
    pub(crate) output: Option<String>,
    pub(crate) input: Option<String>,
    pub(crate) stderr: Option<String>,
}

/// How an example is checked. Working, warning and error examples are all `Run`; their markers
/// tell them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Run,
    Panics,
    NoRun,
    Pending,
}

/// A diagnostic marker: `code` on `line` of the program.
#[derive(Debug)]
pub(crate) struct Marker {
    pub(crate) code: String,
    pub(crate) line: usize,
}

/// What the companion blocks that come next attach to.
enum Open {
    /// Nothing: a companion block here is a fault.
    Nothing,
    /// The example at this index of the page's examples.
    Example(usize),
    /// An example with a fault. Its companions are skipped, so the one fault is not repeated.
    Faulted,
}

/// Reads `text`, a page, into its examples and its faults. A block gets at most one fault, and an
/// example with a fault in it or in a companion block is left out of the examples.
pub(crate) fn read_page(text: &str) -> Page {
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(at, _)| at + 1))
        .collect();
    let line_of = |offset: usize| line_starts.partition_point(|&start| start <= offset);
    let mut reader = Reader {
        examples: Vec::new(),
        dropped: Vec::new(),
        faults: Vec::new(),
        open: Open::Nothing,
    };
    // The code block being read: its start offset, its kind and its text so far.
    let mut block: Option<(usize, CodeBlockKind, String)> = None;
    let mut events = Parser::new_ext(text, Options::ENABLE_TABLES).into_offset_iter();
    for (event, range) in events.by_ref() {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => block = Some((range.start, kind, String::new())),
            // Text outside a code block is inside a paragraph or a heading, whose start
            // already closed the open example.
            Event::Text(part) => {
                if let Some((_, _, body)) = &mut block {
                    body.push_str(&part);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                let (start, kind, body) = block
                    .take()
                    .expect("a code block should end after it starts");
                let line = line_of(start);
                let at_column_0 = line_starts[line - 1] == start && text.as_bytes()[start] == b'`';
                reader.code_block(line, at_column_0, &kind, body);
            }
            // Any other block comes between an example and a companion block.
            Event::Start(Tag::HtmlBlock) => {
                reader.faults.push(Fault {
                    line: line_of(range.start),
                    message: "a raw HTML block is not allowed: the site shows a fence in it, \
                              and the checker cannot see that fence"
                        .to_string(),
                });
                reader.open = Open::Nothing;
            }
            _ => reader.open = Open::Nothing,
        }
    }
    let Reader {
        examples,
        dropped,
        mut faults,
        ..
    } = reader;
    faults.extend(
        events
            .reference_definitions()
            .iter()
            .map(|(_, definition)| Fault {
                line: line_of(definition.span.start),
                message: "a link reference definition is not allowed; use an inline link"
                    .to_string(),
            }),
    );
    faults.sort_by_key(|fault| fault.line);
    let examples = examples
        .into_iter()
        .zip(dropped)
        .filter_map(|(example, dropped)| (!dropped).then_some(example))
        .collect();
    Page { examples, faults }
}

/// What `read_page` has found so far, and what a companion block attaches to.
struct Reader {
    examples: Vec<Example>,
    /// For each example, if a companion block of it has a fault.
    dropped: Vec<bool>,
    faults: Vec<Fault>,
    open: Open,
}

impl Reader {
    /// Reads the code block of `kind` with `body` on Markdown `line`. `at_column_0` says that it
    /// starts with a backtick at column 0.
    fn code_block(&mut self, line: usize, at_column_0: bool, kind: &CodeBlockKind, body: String) {
        let fault = match kind {
            CodeBlockKind::Indented => {
                Some("an indented code block is not checked; use a ``` fence".to_string())
            }
            // A fence in a block quote or a list item starts after the `>` or the
            // item's indent, so it is not at column 0 either.
            CodeBlockKind::Fenced(_) if !at_column_0 => Some(
                "a fence must be ``` at column 0, outside block quotes and list items".to_string(),
            ),
            CodeBlockKind::Fenced(info) => {
                let (name, words) = info.split_once(' ').unwrap_or((info, ""));
                match name {
                    "sisu" => match example(line, words, body) {
                        Ok(found) => {
                            self.examples.push(found);
                            self.dropped.push(false);
                            self.open = Open::Example(self.examples.len() - 1);
                            None
                        }
                        Err(fault) => {
                            self.open = Open::Faulted;
                            self.faults.push(fault);
                            None
                        }
                    },
                    "output" | "input" | "stderr" if words.is_empty() => match self.open {
                        Open::Nothing => Some(format!(
                            "this `{name}` block follows no example; put it directly \
                             after its `sisu` block"
                        )),
                        Open::Faulted => None,
                        Open::Example(index) => {
                            let fault = companion(&mut self.examples[index], name, body);
                            self.dropped[index] |= fault.is_some();
                            fault
                        }
                    },
                    "sh" | "text" if words.is_empty() => {
                        self.open = Open::Nothing;
                        None
                    }
                    _ => Some(format!(
                        "the fence `{info}` is not one of `sisu ...`, `output`, `input`, \
                         `stderr`, `sh` and `text`"
                    )),
                }
            }
        };
        if let Some(message) = fault {
            self.faults.push(Fault { line, message });
        }
    }
}

/// The example of the `sisu` fence on Markdown `line`, with the info words after `sisu` and the
/// program `source`, or its one fault.
fn example(line: usize, words: &str, source: String) -> Result<Example, Fault> {
    let fault = |line, message: &str| Fault {
        line,
        message: message.to_string(),
    };
    let (kind, args) = info(words).map_err(|message| fault(line, &message))?;
    let mut markers = Vec::new();
    let mut first_output = None;
    for (number, comment) in comments(&source) {
        match marker(comment) {
            Some(code) => markers.push(Marker {
                code: code.to_string(),
                line: number,
            }),
            None => {
                first_output.get_or_insert(number);
            }
        }
    }
    let error = markers.iter().any(|marker| marker.code.starts_with('E'));
    if error && matches!(kind, Kind::Panics | Kind::NoRun) {
        return Err(fault(
            line,
            "a panics or no-run example takes only warning markers",
        ));
    }
    if args.is_some() && (error || matches!(kind, Kind::NoRun | Kind::Pending)) {
        return Err(fault(
            line,
            "only a working, warning or panics example takes `args`",
        ));
    }
    if let Some(number) =
        first_output.filter(|_| (error && kind == Kind::Run) || kind == Kind::NoRun)
    {
        return Err(fault(
            line + number,
            "an error or no-run example has no output lines; this trailing comment is not a \
             diagnostic marker",
        ));
    }
    Ok(Example {
        line,
        source,
        kind,
        markers,
        args: args.unwrap_or_default(),
        output: None,
        input: None,
        stderr: None,
    })
}

/// The kind and the arguments that the info `words` after `sisu` give.
fn info(words: &str) -> Result<(Kind, Option<Vec<String>>), String> {
    let mut kind = None;
    let mut args = None;
    let mut rest = words.trim_start();
    while !rest.is_empty() {
        if let Some(value) = rest.strip_prefix("args=") {
            let quoted = value
                .strip_prefix('"')
                .and_then(|value| value.split_once('"'))
                .filter(|(_, tail)| tail.is_empty() || tail.starts_with(' '));
            let Some((list, tail)) = quoted else {
                return Err("write the arguments in double quotes: `args=\"a b\"`".to_string());
            };
            let list = list
                .split(' ')
                .filter(|arg| !arg.is_empty())
                .map(String::from)
                .collect();
            if args.replace(list).is_some() {
                return Err("the word `args` is in the info string two times".to_string());
            }
            rest = tail.trim_start();
            continue;
        }
        let (word, tail) = rest.split_once(' ').unwrap_or((rest, ""));
        let this = match word {
            "pending" => Kind::Pending,
            "panics" => Kind::Panics,
            "no-run" => Kind::NoRun,
            _ => {
                return Err(format!(
                    "unknown word `{word}`: an example takes `pending`, `panics`, `no-run` and \
                     `args=\"...\"`"
                ));
            }
        };
        if kind.replace(this).is_some() {
            return Err("use only one of `pending`, `panics` and `no-run`, one time".to_string());
        }
        rest = tail.trim_start();
    }
    Ok((kind.unwrap_or(Kind::Run), args))
}

/// Attaches the companion block `name` with `body` to `example`, or gives the fault in it.
fn companion(example: &mut Example, name: &str, body: String) -> Option<String> {
    let error = example
        .markers
        .iter()
        .any(|marker| marker.code.starts_with('E'));
    if error || matches!(example.kind, Kind::NoRun | Kind::Pending) {
        return Some(format!(
            "an error, no-run or pending example takes no `{name}` block"
        ));
    }
    let slot = match name {
        "output" => &mut example.output,
        "input" => &mut example.input,
        _ => &mut example.stderr,
    };
    if slot.replace(body).is_some() {
        return Some(format!("the example has a second `{name}` block"));
    }
    None
}

/// Each trailing comment of `source`, with its 1-based program line: the text after the last
/// `//` on a line with code before it, less one space after the `//`.
fn comments(source: &str) -> impl Iterator<Item = (usize, &str)> {
    source.lines().enumerate().filter_map(|(index, line)| {
        if line.trim_start().starts_with("//") {
            return None;
        }
        let (_, comment) = line.rsplit_once("//")?;
        Some((index + 1, comment.strip_prefix(' ').unwrap_or(comment)))
    })
}

/// The code of `comment` when it is exactly a diagnostic marker: `error[Ennnn]` or
/// `warning[Wnnnn]`.
fn marker(comment: &str) -> Option<&str> {
    let code = comment
        .strip_prefix("error[")
        .filter(|code| code.starts_with('E'))
        .or_else(|| {
            comment
                .strip_prefix("warning[")
                .filter(|code| code.starts_with('W'))
        })?
        .strip_suffix(']')?;
    support::is_code(code).then_some(code)
}

/// The verdict on a pending example's `sisu check`, from its exit `code` and its `stderr`: it
/// passes on exit 1 with an `error[` line.
pub(crate) fn pending_result(code: Option<i32>, stderr: &str) -> Result<(), String> {
    match code {
        Some(1) if stderr.lines().any(|line| line.starts_with("error[")) => Ok(()),
        Some(0) => Err(
            "the pending example compiles now: make it a working example (remove `pending`) and \
             give it its output"
                .to_string(),
        ),
        Some(code) => Err(format!(
            "sisu check gives exit code {code} and reports no error:\n{stderr}"
        )),
        None => Err(format!(
            "sisu check gives no exit code (a signal stopped it):\n{stderr}"
        )),
    }
}

/// Writes `example` to `dir/example.sisu` and checks it as its kind says. The error names the
/// program line where it can, and shows what the page expects against what `sisu` gives.
pub(crate) fn check(example: &Example, dir: &Path) -> Result<(), String> {
    fs::write(dir.join("example.sisu"), &example.source)
        .expect("the case directory should be writable");
    let sisu = Path::new(env!("CARGO_BIN_EXE_sisu"));
    if example.kind == Kind::Pending {
        let out = Command::new(sisu)
            .current_dir(dir)
            .args(["check", "example.sisu"])
            .output()
            .expect("sisu should start");
        return pending_result(out.status.code(), &String::from_utf8_lossy(&out.stderr));
    }
    let error = example
        .markers
        .iter()
        .any(|marker| marker.code.starts_with('E'));
    if !example.markers.is_empty() {
        diagnostics(example, sisu, dir, error)?;
        if error {
            return Ok(());
        }
    }
    let exe = dir.join("example");
    let built = support::sisu(dir, "example.sisu", &exe, &[])
        .output()
        .expect("sisu should start");
    let build_stderr = String::from_utf8_lossy(&built.stderr);
    if built.status.code() != Some(0) {
        return Err(format!(
            "sisu build fails ({}):\n{build_stderr}",
            built.status
        ));
    }
    if example.markers.is_empty() && !build_stderr.is_empty() {
        return Err(format!(
            "sisu build warns, and the example has no marker for the warning:\n{build_stderr}"
        ));
    }
    if example.kind == Kind::NoRun {
        return Ok(());
    }
    let input = example.input.clone().unwrap_or_default().into_bytes();
    let args = example.args.iter().map(OsString::from).collect();
    let run = support::Invocation::new(input, args).run(&exe, false);
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    let exit = if example.kind == Kind::Panics { 101 } else { 0 };
    if run.status.code() != Some(exit) {
        return Err(format!(
            "the program should exit {exit}, and it gives {}\nstandard output:\n{stdout}\n\
             standard error:\n{stderr}",
            run.status
        ));
    }
    match &example.output {
        Some(output) if run.stdout != output.as_bytes() => {
            return Err(format!(
                "the standard output is not the `output` block\nexpected:\n{output}\nfound:\n\
                 {stdout}"
            ));
        }
        Some(_) => {}
        None => output_lines(&example.source, &run.stdout)?,
    }
    let expected = example.stderr.as_deref().unwrap_or("");
    if run.stderr != expected.as_bytes() {
        return Err(format!(
            "the standard error is not {}\nexpected:\n{expected}\nfound:\n{stderr}",
            if example.stderr.is_some() {
                "the `stderr` block"
            } else {
                "empty"
            }
        ));
    }
    Ok(())
}

/// Checks that `stdout` is the output lines of `source`, its trailing comments that are not
/// markers, each with a line break.
pub(crate) fn output_lines(source: &str, stdout: &[u8]) -> Result<(), String> {
    let wanted: Vec<(usize, &str)> = comments(source)
        .filter(|(_, comment)| marker(comment).is_none())
        .collect();
    let expected: String = wanted.iter().flat_map(|(_, line)| [*line, "\n"]).collect();
    if stdout == expected.as_bytes() {
        return Ok(());
    }
    let found = String::from_utf8_lossy(stdout);
    let mut lines = found.split_inclusive('\n');
    for (number, want) in &wanted {
        match lines.next() {
            Some(line) if line.strip_suffix('\n') == Some(want) => {}
            Some(line) => {
                return Err(format!(
                    "line {number}: the output line is {want:?}, and the program prints {line:?}"
                ));
            }
            None => {
                return Err(format!(
                    "line {number}: the output line is {want:?}, and the program prints nothing \
                     more"
                ));
            }
        }
    }
    let rest: String = lines.collect();
    if rest.is_empty() {
        return Err(format!("the standard output is not UTF-8: {found:?}"));
    }
    Err(format!(
        "the program prints more than its output lines: {rest:?}"
    ))
}

/// Checks that `sisu check --format json` reports exactly the diagnostics of `example`'s markers,
/// each on its marker's line, and exits 1 when one is an `error`, else 0.
fn diagnostics(example: &Example, sisu: &Path, dir: &Path, error: bool) -> Result<(), String> {
    let out = Command::new(sisu)
        .current_dir(dir)
        .args(["check", "--format", "json", "example.sisu"])
        .output()
        .expect("sisu should start");
    let json = String::from_utf8_lossy(&out.stderr);
    let mut found: Vec<(&str, u64)> = Vec::new();
    let records: Vec<Value> = json
        .lines()
        .map(|line| {
            serde_json::from_str(line)
                .expect("each stderr line of `sisu check --format json` should be JSON")
        })
        .collect();
    for record in &records {
        if record["kind"] == "diagnostic" {
            found.push((
                record["code"]
                    .as_str()
                    .expect("a diagnostic should have a code"),
                record["primary"]["line"]
                    .as_u64()
                    .expect("a diagnostic should have a primary line"),
            ));
        }
    }
    let mut expected: Vec<(&str, u64)> = example
        .markers
        .iter()
        .map(|marker| {
            let line = u64::try_from(marker.line).expect("a line number should fit in a u64");
            (marker.code.as_str(), line)
        })
        .collect();
    found.sort_unstable();
    expected.sort_unstable();
    let list = |pairs: &[(&str, u64)]| {
        pairs
            .iter()
            .map(|(code, line)| format!("{code} on line {line}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if found != expected {
        return Err(format!(
            "the markers give [{}], and sisu check reports [{}]",
            list(&expected),
            list(&found)
        ));
    }
    let exit = i32::from(error);
    if out.status.code() != Some(exit) {
        return Err(format!(
            "sisu check should exit {exit}, and it gives {}",
            out.status
        ));
    }
    Ok(())
}
