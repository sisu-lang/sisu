//! The entry point of the program fuzz target: generates a Sisu program from the input bytes,
//! builds it and its rewritten copy at O0, at `-O2` and with `--rc-checks`, runs each build and
//! panics on a breach of a property (ADR 0033). Properties 1 and 2 are that the program checks
//! clean and compiles to a module LLVM verifies; property 3 is that every build exits 0 or 101,
//! and all three the same way; property 4 is that the rewritten copy runs as the program does;
//! property 5 is that the program prints the known integer answers, and that each call of an
//! interface's requirement prints the value of the method it should reach; property 6 is that a
//! copy of the program where one argument breaks the bound of its type parameter gets E0154 at
//! that argument and no other error.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::os::unix::process::ExitStatusExt;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::Once;
use std::thread;
use std::time::{Duration, Instant};

use crate::codegen::OptLevel;
use crate::diagnostic::{Diagnostic, Severity};
use crate::generate::{self, Answer, BoundSite, Generated, Known};
use crate::report::{Format, Reporter};
use crate::source::LineIndex;
use crate::timings::Timings;
use crate::tir::Program;
use crate::{Backend, compile, fresh_names, front_end, fuzz, run};

/// How long a run may take (decision 5): a program within budget takes milliseconds.
const LIMIT: Duration = Duration::from_secs(10);

/// The three builds of a program, by the flags that make them.
const BUILDS: [(&str, Backend); 3] = [
    (
        "O0",
        Backend {
            level: OptLevel::O0,
            rc_checks: false,
            linker: None,
            allocator: None,
        },
    ),
    (
        "-O2",
        Backend {
            level: OptLevel::O2,
            rc_checks: false,
            linker: None,
            allocator: None,
        },
    ),
    (
        "--rc-checks",
        Backend {
            level: OptLevel::O0,
            rc_checks: true,
            linker: None,
            allocator: None,
        },
    ),
];

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Exit {
    Code(i32),
    Signal(i32),
    TimedOut,
}

/// What a run of one build did.
#[derive(Debug)]
struct Run {
    exit: Exit,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// A breach of property `property`.
#[derive(Debug)]
struct Failure {
    property: u8,
    detail: String,
    /// The broken copy of a failed property 6, for the dump.
    broken: Option<String>,
}

impl Failure {
    fn new(property: u8, detail: String) -> Failure {
        Failure {
            property,
            detail,
            broken: None,
        }
    }
}

/// The input `program` is judging on this thread, for the panic hook.
struct Current {
    generated: Generated,
    /// The input's temporary directory.
    dir: PathBuf,
    /// Set by a property failure before its panic: the property line and the failed builds.
    failure: Option<(String, String)>,
    /// The broken copy that failed property 6.
    broken: Option<String>,
}

thread_local! {
    static CURRENT: RefCell<Option<Current>> = const { RefCell::new(None) };
}

/// Generates a program from `data`, builds it and its rewritten copy at O0, -O2 and with
/// --rc-checks against `runtime`, runs each build and panics on a breach of a property.
/// The caller runs it on a `STACK_SIZE` thread, as for `fuzz_front_end`.
#[cfg_attr(
    not(feature = "fuzz"),
    expect(
        unreachable_pub,
        reason = "`lib.rs` re-exports it under the fuzz feature"
    )
)]
pub fn program(data: &[u8], runtime: &Path) {
    program_in(data, runtime, &std::env::temp_dir());
}

/// `program`, with the input's temporary directory made in `tmp`.
fn program_in(data: &[u8], runtime: &Path, tmp: &Path) {
    install_hook();
    let generated = generate::generate(data);
    let dir = run::private_dir(tmp, fresh_names(""))
        .unwrap_or_else(|e| panic!("the fuzzer should make its temporary directory: {e}"));
    let _clear = Clear(dir.clone());
    CURRENT.set(Some(Current {
        generated,
        dir,
        failure: None,
        broken: None,
    }));
    let judged = CURRENT.with_borrow(|current| {
        let current = current
            .as_ref()
            .expect("`program` should have set the input");
        judge(&current.generated, runtime, &current.dir)
    });
    if let Err((failure, builds)) = judged {
        let property = failure.property;
        CURRENT.with_borrow_mut(|current| {
            let current = current
                .as_mut()
                .expect("`program` should have set the input");
            current.failure = Some((format!("property {property}: {}", failure.detail), builds));
            current.broken = failure.broken;
        });
        panic!("program fuzzer: property {property} failed");
    }
}

/// Clears this thread's input and removes its directory when `program` returns or unwinds.
struct Clear(PathBuf);

impl Drop for Clear {
    fn drop(&mut self) {
        CURRENT.set(None);
        // The panic hook may have removed it already, and a leftover temporary directory must
        // not turn a pass into a panic, or a panic into an abort.
        drop(fs::remove_dir_all(&self.0));
    }
}

/// Installs, once, a panic hook in front of the one already set (libFuzzer's aborts, so code
/// after a compiler panic never runs): on a thread inside `program`, it prints the record of
/// decision 9, writes it to `SISU_FUZZ_DUMP` when that is set, removes the input's directory,
/// then calls the earlier hook.
fn install_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let earlier = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // `try_with`: the thread may be tearing down its locals.
            let _ = CURRENT.try_with(|current| {
                if let Ok(current) = current.try_borrow()
                    && let Some(current) = current.as_ref()
                {
                    leave_record(current, info);
                }
            });
            earlier(info);
        }));
    });
}

/// Prints the failed property, or `compiler panic`, then the input, then the failed builds or the
/// compiler's panic `info`; writes the same to `SISU_FUZZ_DUMP` when it names a directory; and
/// removes the input's directory.
fn leave_record(current: &Current, info: &PanicHookInfo<'_>) {
    let Generated {
        program,
        rewritten,
        stdin,
        ..
    } = &current.generated;
    let panic;
    let (first, builds) = if let Some((first, builds)) = &current.failure {
        (first.as_str(), builds.as_str())
    } else {
        panic = format!("{info}\n");
        ("compiler panic", panic.as_str())
    };
    let mut record = format!("{first}\n");
    section(&mut record, "program", program.as_bytes());
    section(&mut record, "rewritten", rewritten.as_bytes());
    section(&mut record, "stdin", stdin.as_bytes());
    record.push_str(builds);
    // A hook must not panic, so a failed write is only reported.
    drop(io::stderr().write_all(record.as_bytes()));
    if let Some(dump) = std::env::var_os("SISU_FUZZ_DUMP") {
        let dump = Path::new(&dump);
        let failure = format!("{first}\n{builds}");
        let written = fs::create_dir_all(dump).and_then(|()| {
            let broken = current.broken.as_deref().map(|text| ("broken.sisu", text));
            [
                ("program.sisu", program.as_str()),
                ("rewritten.sisu", rewritten.as_str()),
                ("stdin.txt", stdin.as_str()),
                ("failure.txt", failure.as_str()),
            ]
            .into_iter()
            .chain(broken)
            .try_for_each(|(name, text)| fs::write(dump.join(name), text))
        });
        if let Err(e) = written {
            drop(writeln!(
                io::stderr(),
                "cannot write {}: {e}",
                dump.display()
            ));
        }
    }
    drop(fs::remove_dir_all(&current.dir));
}

/// Appends `text` to `out` under the line `==> <name>`, ending in a line break.
fn section(out: &mut String, name: &str, text: &[u8]) {
    out.push_str("==> ");
    out.push_str(name);
    out.push('\n');
    out.push_str(&String::from_utf8_lossy(text));
    if !text.is_empty() && !text.ends_with(b"\n") {
        out.push('\n');
    }
}

/// Properties 1 to 6 for `g`, built in `dir`. A failure comes with the record of the builds
/// that showed it.
fn judge(g: &Generated, runtime: &Path, dir: &Path) -> Result<(), (Failure, String)> {
    let program = checked("program", &g.program).map_err(|f| (f, String::new()))?;
    let rewritten = checked("rewritten", &g.rewritten).map_err(|f| (f, String::new()))?;
    // Only checks, so before the builds that take most of the time.
    judge_bounds(g).map_err(|f| (f, String::new()))?;
    let stdin = dir.join("stdin.txt");
    fs::write(&stdin, &g.stdin).expect("the fuzzer should write the stdin to its own directory");
    let runs = judge_source("program", &g.program, &program, runtime, dir, &stdin)?;
    let copies = judge_source("rewritten", &g.rewritten, &rewritten, runtime, dir, &stdin)?;
    judge_pair(&runs[0], &copies[0])
        .map_err(|f| (f, record("program", &runs) + &record("rewritten", &copies)))?;
    judge_known(&runs[0], &g.known).map_err(|f| (f, record("program", &runs)))?;
    judge_marked(&runs, &g.expected).map_err(|f| (f, record("program", &runs)))
}

/// Property 3 for `source`, checked as `program`: its three builds in `dir`, run with `stdin`,
/// which it gives back.
fn judge_source(
    name: &str,
    source: &str,
    program: &Program,
    runtime: &Path,
    dir: &Path,
    stdin: &Path,
) -> Result<[Run; 3], (Failure, String)> {
    #[cfg_attr(not(test), expect(unused_mut, reason = "only a test forces a failure"))]
    let mut runs = builds(name, source, program, runtime, dir, stdin, LIMIT);
    #[cfg(test)]
    if FORCE.get() == Some(Force::Stdout) {
        runs[1].stdout = b"forced".to_vec();
    }
    match judge_builds(&runs) {
        Ok(()) => Ok(runs),
        Err(f) => Err((f, record(name, &runs))),
    }
}

/// `diagnostics` of `source`, named `name`, in text form.
fn render(name: &str, source: &str, diagnostics: &[Diagnostic]) -> String {
    let mut out = Vec::new();
    let mut reporter = Reporter::new(&mut out, Format::Text, false);
    reporter.diagnostics(&format!("{name}.sisu"), source, diagnostics);
    reporter.finish();
    String::from_utf8_lossy(&out).into_owned()
}

/// Property 6: in the copy of the program where `site` breaks the bound of its type parameter, the
/// checker reports an error, each error is E0154, and each starts at the replaced argument. Any
/// other error is a cascade, as #283 was. Warnings do not count: a program with an error has none
/// of its own.
fn judge_site(g: &Generated, site: &BoundSite) -> Result<(), Failure> {
    #[cfg_attr(not(test), expect(unused_mut, reason = "only a test forces a failure"))]
    let mut broken = g.broken(site);
    #[cfg(test)]
    if FORCE.get() == Some(Force::Conforming) {
        // The argument as it was, which conforms: the property must fail.
        broken = g.program.clone() + generate::NO_BOUND_CLASS;
    }
    let (_, diagnostics) = front_end::front_end(&broken);
    let mut errors = diagnostics
        .iter()
        .filter(|d| d.severity() == Severity::Error)
        .peekable();
    let what = if errors.peek().is_none() {
        "reports no error"
    } else if errors.all(|d| d.code().as_str() == "E0154" && d.span().start() == site.at.start) {
        return Ok(());
    } else {
        "reports an error that is not E0154 at the argument"
    };
    let (line, col) = LineIndex::new(&broken).line_col(&broken, site.at.start);
    let replaced = &g.program[site.at.clone()];
    Err(Failure {
        broken: Some(broken.clone()),
        ..Failure::new(
            6,
            format!(
                "the checker {what}, where `{}` replaces `{replaced}` at {line}:{col}, \
                 to break the bound `{}`\n{}",
                site.broken,
                site.bound,
                render("broken", &broken, &diagnostics),
            ),
        )
    })
}

/// Property 6 for every bound site of `g`.
fn judge_bounds(g: &Generated) -> Result<(), Failure> {
    g.bound_sites
        .iter()
        .try_for_each(|site| judge_site(g, site))
}

/// Properties 1 and 2: `source` checks with no error, W0010 or W0011, and its MIR certifies and its
/// module verifies, without and with `--rc-checks` (`fuzz::assert_compiles` panics if not).
fn checked(name: &str, source: &str) -> Result<Program, Failure> {
    // Every generated function returns and every generated loop ends, so none may call itself
    // on every path, and no loop condition may stay unchanged.
    let endless = |d: &Diagnostic| matches!(d.code().as_str(), "W0010" | "W0011");
    let (program, diagnostics) = front_end::front_end(source);
    let failure = |what: String| {
        // Other warnings are allowed, so the detail shows only what fails the property.
        let failing: Vec<Diagnostic> = diagnostics
            .iter()
            .filter(|d| d.severity() == Severity::Error || endless(d))
            .cloned()
            .collect();
        let rendered = render(name, source, &failing);
        Failure::new(1, format!("{what}\n{rendered}"))
    };
    let Some(program) = program else {
        return Err(failure(format!("the checker rejects the {name}")));
    };
    if let Some(d) = diagnostics.iter().find(|d| endless(d)) {
        let code = d.code().as_str();
        return Err(failure(format!("the checker reports {code} on the {name}")));
    }
    fuzz::assert_compiles(&program, source);
    Ok(program)
}

/// Builds `program`, checked from `source`, three ways into `dir`, and runs each build with
/// `stdin` for at most `limit`.
fn builds(
    name: &str,
    source: &str,
    program: &Program,
    runtime: &Path,
    dir: &Path,
    stdin: &Path,
    limit: Duration,
) -> [Run; 3] {
    std::array::from_fn(|i| {
        let exe = dir.join(format!("{name}-{i}"));
        build(name, source, program, &BUILDS[i].1, runtime, &exe);
        run(&exe, stdin, dir, limit)
    })
}

/// Builds `program`, checked from `source`, with `backend` into the executable `exe`, going
/// through an object file in `exe`'s directory.
fn build(
    name: &str,
    source: &str,
    program: &Program,
    backend: &Backend,
    runtime: &Path,
    exe: &Path,
) {
    #[cfg(test)]
    assert!(
        FORCE.get() != Some(Force::CompilerPanic),
        "a compiler panic forced by a test"
    );
    let dir = exe
        .parent()
        .expect("an executable should be in a directory");
    compile(
        program,
        &format!("{name}.sisu"),
        source,
        runtime,
        exe,
        dir,
        backend,
        &mut Timings::off(),
    )
    .unwrap_or_else(|e| panic!("the fuzzer should build the {name}: {e}"));
}

/// Runs `exe` with `stdin` as its stdin, its stdout and stderr in files in `dir`, and kills it
/// once it has run for `limit`.
fn run(exe: &Path, stdin: &Path, dir: &Path, limit: Duration) -> Run {
    let (out, err) = (dir.join("stdout"), dir.join("stderr"));
    let open = |path: &Path| File::open(path).expect("the stdin file should be readable");
    let create =
        |path: &Path| File::create(path).expect("the fuzzer should create files in its directory");
    let mut child = Command::new(exe)
        .stdin(open(stdin))
        .stdout(create(&out))
        .stderr(create(&err))
        .spawn()
        .unwrap_or_else(|e| panic!("{} should start: {e}", exe.display()));
    let start = Instant::now();
    let exit = loop {
        if let Some(status) = child
            .try_wait()
            .expect("the fuzzer should wait for its child")
        {
            break exit(status);
        }
        if start.elapsed() >= limit {
            child.kill().expect("the fuzzer should kill its child");
            child.wait().expect("the fuzzer should reap its child");
            break Exit::TimedOut;
        }
        thread::sleep(Duration::from_millis(1));
    };
    let read = |path: &Path| fs::read(path).expect("the run's output should be readable");
    Run {
        exit,
        stdout: read(&out),
        stderr: read(&err),
    }
}

/// How `status` ended a run that did not time out.
fn exit(status: ExitStatus) -> Exit {
    match (status.code(), status.signal()) {
        (Some(code), _) => Exit::Code(code),
        (None, Some(signal)) => Exit::Signal(signal),
        (None, None) => unreachable!("a Unix process should end with a code or by a signal"),
    }
}

/// Property 3: each build exits 0 or with a Sisu panic, 101, so the `--rc-checks` build reports
/// no count fault or leak (102); and the three agree on exit, stdout and stderr exactly.
fn judge_builds(runs: &[Run; 3]) -> Result<(), Failure> {
    let fail = |detail: String| Err(Failure::new(3, detail));
    for ((build, _), run) in BUILDS.iter().zip(runs) {
        if !matches!(run.exit, Exit::Code(0 | 101)) {
            return fail(format!("the {build} build exits with {:?}", run.exit));
        }
    }
    let first = &runs[0];
    for ((build, _), run) in BUILDS.iter().zip(runs).skip(1) {
        let differs = [
            ("exit", first.exit != run.exit),
            ("stdout", first.stdout != run.stdout),
            ("stderr", first.stderr != run.stderr),
        ];
        if let Some((what, _)) = differs.iter().find(|(_, differs)| *differs) {
            return fail(format!("the O0 and {build} builds differ in {what}"));
        }
    }
    Ok(())
}

/// Property 4: the program and its rewritten copy, each run the same way, exit the same way and
/// print the same, but for where a panic happened.
fn judge_pair(program: &Run, rewritten: &Run) -> Result<(), Failure> {
    let differs = [
        ("exit", program.exit != rewritten.exit),
        ("stdout", program.stdout != rewritten.stdout),
        ("stderr", unlocated(program) != unlocated(rewritten)),
    ];
    match differs.iter().find(|(_, differs)| *differs) {
        Some((what, _)) => Err(Failure::new(
            4,
            format!("the program and its rewritten copy differ in {what}"),
        )),
        None => Ok(()),
    }
}

/// The prefix of the runtime's panic line, before its `<file>:<line>:<col>: <message>`.
const PANIC: &str = "sisu: panic at ";

/// `run`'s stderr, with the `<file>:<line>:<col>: ` taken out of the line of each `dbg` and of a
/// panic. The panic's is only from the last line, and only when the run exited 101. Any other
/// line compares as written.
fn unlocated(run: &Run) -> Vec<u8> {
    let stderr = &run.stderr;
    let body = stderr.strip_suffix(b"\n").unwrap_or(stderr);
    let last = body
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |at| at + 1);
    let after = last + PANIC.len();
    let location = body[last..]
        .starts_with(PANIC.as_bytes())
        .then(|| body[after..].windows(2).position(|pair| pair == b": "))
        .flatten();
    let panic = match location {
        Some(end) if run.exit == Exit::Code(101) => {
            [&stderr[..after], &stderr[after + end + 2..]].concat()
        }
        _ => stderr.clone(),
    };
    panic
        .split_inclusive(|&b| b == b'\n')
        .flat_map(|line| &line[dbg_location(line)..])
        .copied()
        .collect()
}

/// The length of the `<file>.sisu:<line>:<col>: ` that starts `line` when `dbg` wrote it, else 0.
/// The file name holds no space, so a panic's line or a line of text that `eprint` wrote does not
/// start with one. A generated program's text holds no `:`.
fn dbg_location(line: &[u8]) -> usize {
    let digits = |from: usize| {
        line.get(from..)
            .unwrap_or_default()
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let Some(name) = line.windows(6).position(|window| window == b".sisu:") else {
        return 0;
    };
    if line[..name].iter().any(u8::is_ascii_whitespace) {
        return 0;
    }
    let row = name + 6;
    let col = row + digits(row) + 1;
    let end = col + digits(col);
    let well_formed = digits(row) > 0 && line.get(col - 1) == Some(&b':') && digits(col) > 0;
    if well_formed && line[end..].starts_with(b": ") {
        end + 2
    } else {
        0
    }
}

/// Property 5: `run`, the program's O0 run, prints each known value site's line as the first
/// lines of its stdout, in order; and a panic site, when its `@k<n> panics` line is the last,
/// panics with the expected message, while a run that does not reach it panics before it.
/// Only those exact lines are read, so another line that starts with `@` changes nothing.
fn judge_known(run: &Run, known: &[Known]) -> Result<(), Failure> {
    let fail = |detail: String| Err(Failure::new(5, detail));
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    let mut lines = stdout.lines();
    for site in known {
        let marker = &site.marker;
        match &site.answer {
            Answer::Value(value) => {
                let line = format!("@{marker} {value}");
                if lines.next() != Some(line.as_str()) {
                    return fail(format!("stdout should print `{line}` here"));
                }
            }
            Answer::Panic(message) => {
                if run.exit != Exit::Code(101) {
                    return fail(format!(
                        "the panic site {marker} should panic, or one before it"
                    ));
                }
                let reached = stdout.lines().last() == Some(generate::panic_line(marker).as_str());
                let panicked = stderr.lines().last().is_some_and(|last| {
                    last.starts_with(PANIC) && last.ends_with(&format!(": {message}"))
                });
                if reached && !panicked {
                    return fail(format!(
                        "the panic site {marker} should panic with `{message}`"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Property 5 for the calls of requirements: in each build of the program, the lines of stdout
/// that start with `@d` are the first lines of `expected`, in order. A run that returns or panics
/// before `shapes` prints none, and one that stops inside it prints a prefix, so a shorter list
/// passes. A wrong value, a wrong order or an extra line fails, and the failure gives the first
/// line that differs and the line expected there.
fn judge_marked(runs: &[Run], expected: &[String]) -> Result<(), Failure> {
    for ((build, _), run) in BUILDS.iter().zip(runs) {
        let stdout = String::from_utf8_lossy(&run.stdout);
        let printed = stdout.lines().filter(|line| line.starts_with("@d"));
        for (at, line) in printed.enumerate() {
            let detail = match expected.get(at) {
                Some(want) if want == line => continue,
                Some(want) => format!(
                    "the {build} build should print `{want}` as marked line {}, and it prints `{line}`",
                    at + 1
                ),
                None => format!(
                    "the {build} build prints `{line}` as marked line {}, and `shapes` makes only {}",
                    at + 1,
                    expected.len()
                ),
            };
            return Err(Failure::new(5, detail));
        }
    }
    Ok(())
}

/// The builds of `name` in `runs`, each as its exit, stdout and stderr under `==>` lines.
fn record(name: &str, runs: &[Run; 3]) -> String {
    let mut out = String::new();
    for ((build, _), run) in BUILDS.iter().zip(runs) {
        let _ = writeln!(out, "==> {name} {build}: {:?}", run.exit);
        section(&mut out, &format!("{name} {build} stdout"), &run.stdout);
        section(&mut out, &format!("{name} {build} stderr"), &run.stderr);
    }
    out
}

/// A failure a test forces on `program`.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Force {
    /// The program's -O2 run prints something else.
    Stdout,
    /// The build step panics, as a compiler bug would.
    CompilerPanic,
    /// Each bound site's replacement conforms to its bound.
    Conforming,
}

#[cfg(test)]
thread_local! {
    static FORCE: std::cell::Cell<Option<Force>> = const { std::cell::Cell::new(None) };
}

/// `target/debug/libsisu_runtime_mimalloc.a`, the default allocator's runtime, which
/// nextest's setup script builds: the test binary is `target/debug/deps/<name>` (decision 7).
#[cfg(test)]
fn runtime() -> PathBuf {
    let exe = std::env::current_exe().expect("the test binary should know its path");
    exe.parent()
        .and_then(Path::parent)
        .expect("the test binary should be two levels inside the target directory")
        .join("libsisu_runtime_mimalloc.a")
}

/// What `source` writes to its standard output, built at O0 and run once. For the tests of the
/// generator, which check the text of values.
#[cfg(test)]
pub(crate) fn stdout_of(source: &str) -> Vec<u8> {
    let dir = run::private_dir(&std::env::temp_dir(), fresh_names("-test"))
        .expect("the test should make its directory");
    let program = checked("program", source).expect("the test's program should check clean");
    let exe = dir.join("program");
    build("program", source, &program, &BUILDS[0].1, &runtime(), &exe);
    let stdin = dir.join("stdin.txt");
    fs::write(&stdin, "").expect("the test should write the stdin");
    let run = run(&exe, &stdin, &dir, LIMIT);
    drop(fs::remove_dir_all(&dir));
    assert_eq!(run.exit, Exit::Code(0), "the program should exit 0");
    run.stdout
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ops::Range;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::ExitStatusExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::time::Duration;
    use std::{env, fs, thread};

    use super::dbg_location;
    use super::{
        Answer, BUILDS, Exit, FORCE, Force, Known, LIMIT, Run, build, builds, checked, judge,
        judge_bounds, judge_builds, judge_known, judge_marked, judge_pair, judge_source, program,
        program_in, run, runtime,
    };
    use crate::fresh_names;
    use crate::generate::{BoundSite, Generated, coverage, generate};

    /// A directory of the test's own, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Scratch {
            Scratch(
                run::private_dir(&env::temp_dir(), fresh_names("-test"))
                    .expect("the test should make its directory"),
            )
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            drop(fs::remove_dir_all(&self.0));
        }
    }

    /// `f` on a thread with a `STACK_SIZE` stack, as `program`'s contract asks.
    fn on_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
        thread::scope(|scope| {
            thread::Builder::new()
                .stack_size(crate::STACK_SIZE)
                .spawn_scoped(scope, f)
                .expect("the OS should start a thread with a 64 MiB stack")
                .join()
                .expect("the test's thread should not panic")
        })
    }

    /// The three builds of `source`, each run with `stdin` for at most `limit`.
    fn runs_of(source: &str, stdin: &str, limit: Duration) -> [Run; 3] {
        let scratch = Scratch::new();
        let program = checked("program", source).expect("the test's program should check clean");
        let stdin_path = scratch.0.join("stdin.txt");
        fs::write(&stdin_path, stdin).expect("the test should write the stdin");
        builds(
            "program",
            source,
            &program,
            &runtime(),
            &scratch.0,
            &stdin_path,
            limit,
        )
    }

    fn exits(runs: &[Run; 3]) -> [Exit; 3] {
        runs.each_ref().map(|run| run.exit)
    }

    #[test]
    fn a_clean_program_passes() {
        let runs = runs_of("fn main() {\n    print(\"hi\")\n}\n", "", LIMIT);
        assert_eq!(exits(&runs), [Exit::Code(0); 3]);
        for run in &runs {
            assert_eq!(run.stdout, b"hi\n");
            assert_eq!(run.stderr, b"");
        }
        judge_builds(&runs).expect("a clean program should pass property 3");
    }

    #[test]
    fn a_panic_passes_when_every_build_agrees() {
        let runs = runs_of("fn main() {\n    panic(\"x\")\n}\n", "", LIMIT);
        assert_eq!(exits(&runs), [Exit::Code(101); 3]);
        judge_builds(&runs).expect("a panic in every build should pass property 3");
    }

    /// Review Focus 2: the overflow's value is never used, and `-O2` must keep its panic.
    #[test]
    fn unused_overflow_panics_in_every_build() {
        let source =
            "fn f(a: i64) -> i64 {\n    a * a\n}\n\nfn main() {\n    let _ = f(4_000_000_000)\n}\n";
        let runs = runs_of(source, "", LIMIT);
        assert_eq!(exits(&runs), [Exit::Code(101); 3]);
        judge_builds(&runs).expect("every build should panic the same way");
    }

    /// Review Focus 4: `read_line` past the end of the stdin is `None` in every build.
    #[test]
    fn stdin_past_its_end_agrees() {
        let line = "    print(read_line() ?? \"none\")\n";
        let source = format!("fn main() {{\n{}}}\n", line.repeat(3));
        let runs = runs_of(&source, "a\n", LIMIT);
        assert_eq!(exits(&runs), [Exit::Code(0); 3]);
        for run in &runs {
            assert_eq!(run.stdout, b"a\nnone\nnone\n");
        }
        judge_builds(&runs).expect("every build should read the same");
    }

    /// Review Focus 5: an output of 20,000 lines (about 109 KB) is compared whole, and a difference
    /// in the last byte still counts.
    #[test]
    fn large_output_should_be_compared_exactly() {
        let source = "fn main() {\n    let mut i = 0\n    while i < 20_000 {\n        print(i)\n        i += 1\n    }\n}\n";
        let mut runs = runs_of(source, "", LIMIT);
        assert_eq!(exits(&runs), [Exit::Code(0); 3]);
        for run in &runs {
            let text = String::from_utf8_lossy(&run.stdout);
            assert_eq!(text.lines().count(), 20_000);
            assert_eq!(text.lines().last(), Some("19999"));
        }
        judge_builds(&runs).expect("every build should print the same");
        let last = runs[2].stdout.len() - 2;
        runs[2].stdout[last] = b'8';
        let failure = judge_builds(&runs).expect_err("the last digit differs");
        assert_eq!(failure.property, 3);
    }

    #[test]
    fn a_hang_fails_property_3() {
        let source = "fn main() {\n    while true {\n    }\n}\n";
        let scratch = Scratch::new();
        let program = checked("program", source).expect("the test's program should check clean");
        let exe = scratch.0.join("program");
        build("program", source, &program, &BUILDS[0].1, &runtime(), &exe);
        let stdin = scratch.0.join("stdin.txt");
        fs::write(&stdin, "").expect("the test should write the stdin");
        let hung = run(&exe, &stdin, &scratch.0, Duration::from_millis(200));
        assert_eq!(hung.exit, Exit::TimedOut);
        let failure = judge_builds(&[hung, clean(), clean()]).expect_err("a hang should fail");
        assert_eq!(failure.property, 3);
    }

    /// A run past its limit is killed and reaped: no zombie is left behind.
    #[test]
    fn a_timed_out_child_is_reaped() {
        let scratch = Scratch::new();
        let exe = scratch.0.join("hang.sh");
        // Left alive, the script ends by writing `done`: a child that was not killed shows.
        let done = scratch.0.join("done");
        let script = format!("#!/bin/sh\necho $$\nsleep 10\ntouch '{}'\n", done.display());
        fs::write(&exe, script).expect("the test should write it");
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700))
            .expect("the test should make the script executable");
        let stdin = scratch.0.join("stdin.txt");
        fs::write(&stdin, "").expect("the test should write the stdin");
        let mut hung = run(&exe, &stdin, &scratch.0, Duration::from_millis(200));
        if hung.stdout.is_empty() {
            // A loaded machine can take longer than 200 ms to start `sh`: one more, longer try.
            hung = run(&exe, &stdin, &scratch.0, Duration::from_secs(2));
        }
        assert_eq!(hung.exit, Exit::TimedOut);
        let pid = String::from_utf8_lossy(&hung.stdout).trim().to_string();
        assert!(
            !pid.is_empty(),
            "the script should print its PID before it is killed"
        );
        // `kill -0` succeeds on a zombie, so it fails only once the child is reaped.
        let alive = Command::new("sh")
            .args(["-c", &format!("kill -0 {pid} 2>/dev/null")])
            .status()
            .expect("sh should run");
        assert!(!alive.success(), "process {pid} should be gone");
        assert!(!done.exists(), "process {pid} should have been killed");
    }

    /// A run that exits 0 and prints `1`.
    fn clean() -> Run {
        Run {
            exit: Exit::Code(0),
            stdout: b"1\n".to_vec(),
            stderr: Vec::new(),
        }
    }

    /// A change to three runs.
    type Change = fn(&mut [Run; 3]);

    #[test]
    fn judge_builds_rejects_any_difference() {
        judge_builds(&[clean(), clean(), clean()]).expect("equal clean runs should pass");
        // Each case differs from three clean runs in one thing: one field of one run, or the exit
        // of all three, so only the exit rule can catch it.
        let cases: [(&str, Change); 5] = [
            ("exit", |runs| runs[1].exit = Exit::Code(101)),
            ("stdout", |runs| runs[2].stdout = b"2\n".to_vec()),
            ("stderr", |runs| runs[1].stderr = b"x\n".to_vec()),
            ("count fault", |runs| {
                for run in runs {
                    run.exit = Exit::Code(102);
                }
            }),
            ("segfault", |runs| {
                for run in runs {
                    run.exit = Exit::Signal(11);
                }
            }),
        ];
        for (case, change) in cases {
            let mut runs = [clean(), clean(), clean()];
            change(&mut runs);
            let failure = judge_builds(&runs).expect_err(case);
            assert_eq!(failure.property, 3, "{case}");
        }
    }

    /// Either copy failing to check fails property 1, before anything is built.
    #[test]
    fn a_rejected_program_should_fail_property_1() {
        let clean = "fn main() {\n    print(1)\n}\n";
        let rejected = "fn main() {\n    print(x)\n}\n";
        for (program, rewritten) in [(rejected, clean), (clean, rejected)] {
            let g = Generated {
                program: program.to_string(),
                rewritten: rewritten.to_string(),
                stdin: String::new(),
                known: Vec::new(),
                expected: Vec::new(),
                bound_sites: Vec::new(),
                cost: 0,
                plain: String::new(),
                made: Vec::new(),
            };
            let scratch = Scratch::new();
            let (failure, builds) =
                judge(&g, &runtime(), &scratch.0).expect_err("an unknown name should fail");
            assert_eq!((failure.property, builds.as_str()), (1, ""), "{rewritten}");
        }
    }

    /// The program has no error, so only its W0010 can fail property 1.
    #[test]
    fn checked_should_fail_property_1_when_a_function_calls_itself_on_every_path() {
        let Err(failure) = checked("program", "fn _f() {\n    _f()\n}\nfn main() {}\n") else {
            panic!("a function that calls itself on every path should fail");
        };
        assert_eq!(failure.property, 1);
    }

    /// The program has no error, so only its W0011 can fail property 1.
    #[test]
    fn checked_should_fail_property_1_when_a_loop_condition_never_changes() {
        let source = "fn _f(n: i64) {\n    while n > 0 { print(n) }\n}\nfn main() {}\n";
        let Err(failure) = checked("program", source) else {
            panic!("a loop whose condition never changes should fail");
        };
        assert_eq!(failure.property, 1);
    }

    #[test]
    fn a_signal_is_its_number() {
        let scratch = Scratch::new();
        let exe = scratch.0.join("segfault.sh");
        fs::write(&exe, "#!/bin/sh\nkill -SEGV $$\n").expect("the test should write the script");
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700))
            .expect("the test should make the script executable");
        let stdin = scratch.0.join("stdin.txt");
        fs::write(&stdin, "").expect("the test should write the stdin");
        assert_eq!(run(&exe, &stdin, &scratch.0, LIMIT).exit, Exit::Signal(11));
    }

    /// A run that exits with `code` and prints `stdout` and `stderr`.
    fn ran(code: i32, stdout: &str, stderr: &str) -> Run {
        Run {
            exit: Exit::Code(code),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    /// The property `judge_pair` finds broken, 0 for none.
    fn pair(program: &Run, rewritten: &Run) -> u8 {
        judge_pair(program, rewritten).map_or_else(|f| f.property, |()| 0)
    }

    /// Review Focus 3: the copies' panics are at different places, with the same message,
    /// after a line the program printed to stderr.
    #[test]
    fn pair_ignores_panic_locations() {
        let program = ran(
            101,
            "1\n",
            "e\nsisu: panic at input.sisu:3:5: integer overflow\n",
        );
        let rewritten = ran(
            101,
            "1\n",
            "e\nsisu: panic at input.sisu:17:19: integer overflow\n",
        );
        assert_eq!(pair(&program, &rewritten), 0);
    }

    /// Property 4: the line of a `dbg` loses its `<file>:<line>:<col>: `, in every line and in
    /// both files; its text and its value still compare.
    #[test]
    fn pair_should_ignore_dbg_locations_and_compare_the_rest() {
        let at = |file: &str, place: &str, text: &str| {
            format!("{file}.sisu:{place}: {text} = 1\nplain\n")
        };
        let program = ran(
            0,
            "",
            &(at("program", "3:5", "f(x)") + &at("program", "4:9", "y")),
        );
        let moved = ran(
            0,
            "",
            &(at("rewritten", "13:25", "f(x)") + &at("rewritten", "9:1", "y")),
        );
        assert_eq!(pair(&program, &moved), 0);
        let text = ran(
            0,
            "",
            &(at("rewritten", "13:25", "g(x)") + &at("rewritten", "9:1", "y")),
        );
        assert_eq!(pair(&program, &text), 4, "the source text of a `dbg`");
        let value = ran(
            0,
            "",
            &(at("rewritten", "13:25", "f(x)") + &at("rewritten", "9:1", "y"))
                .replace("= 1", "= 2"),
        );
        assert_eq!(pair(&program, &value), 4, "the value of a `dbg`");
    }

    /// A line is a `dbg` line only when it starts with a `.sisu` file name, a line and a column,
    /// and `: `. Any other line compares as written, an `eprint` of the same shape included.
    #[test]
    fn dbg_location_should_take_only_the_prefix_of_a_dbg_line() {
        for (line, length) in [
            ("program.sisu:3:5: f(x) = 1", 18),
            ("a.sisu:10:200: x = 1\n", 15),
            ("program.sisu:3:5 f(x) = 1", 0),
            ("program.sisu:3: f(x) = 1", 0),
            ("program.sisu::5: f(x) = 1", 0),
            ("program.sisu:3:: f(x) = 1", 0),
            ("a b.sisu:3:5: x = 1", 0),
            ("sisu: panic at program.sisu:3:5: overflow", 0),
            ("plain", 0),
            ("", 0),
            ("program.sisu:3", 0),
            ("a.sisu:", 0),
            ("x.sisu:3:", 0),
            ("x.sisu:3:5", 0),
            ("x.sisu:3:5:", 0),
            ("x.sisu:3:5: ", 12),
        ] {
            assert_eq!(dbg_location(line.as_bytes()), length, "{line:?}");
        }
    }

    #[test]
    fn pair_rejects_a_different_panic_message() {
        let program = ran(101, "", "sisu: panic at input.sisu:3:5: integer overflow\n");
        let rewritten = ran(101, "", "sisu: panic at input.sisu:3:5: division by zero\n");
        assert_eq!(pair(&program, &rewritten), 4);
    }

    /// Only a panic's own line loses its location: an `eprint` of the same text, or a line
    /// before the last, compares as written.
    #[test]
    fn pair_compares_ordinary_stderr_exactly() {
        let at = |place: &str| format!("sisu: panic at input.sisu:{place}: hello\n");
        let printed = (ran(0, "", &at("3:5")), ran(0, "", &at("7:9")));
        assert_eq!(pair(&printed.0, &printed.1), 4);
        let panic = "sisu: panic at input.sisu:9:1: integer overflow\n";
        let earlier = (
            ran(101, "", &(at("3:5") + panic)),
            ran(101, "", &(at("7:9") + panic)),
        );
        assert_eq!(pair(&earlier.0, &earlier.1), 4);
    }

    #[test]
    fn pair_rejects_different_stdout() {
        assert_eq!(pair(&ran(0, "1\n", ""), &ran(0, "2\n", "")), 4);
    }

    #[test]
    fn pair_rejects_a_different_exit() {
        let panic = "sisu: panic at input.sisu:3:5: integer overflow\n";
        assert_eq!(pair(&ran(0, "", panic), &ran(101, "", panic)), 4);
    }

    fn value(marker: &str, value: &str) -> Known {
        Known {
            marker: marker.to_string(),
            answer: Answer::Value(value.to_string()),
        }
    }

    fn panics(marker: &str, message: &'static str) -> Known {
        Known {
            marker: marker.to_string(),
            answer: Answer::Panic(message),
        }
    }

    /// The property `judge_known` finds broken, 0 for none.
    fn known(run: &Run, known: &[Known]) -> u8 {
        judge_known(run, known).map_or_else(|f| f.property, |()| 0)
    }

    #[test]
    fn known_value_matches() {
        let site = [value("k0", "42")];
        assert_eq!(known(&ran(0, "@k0 42\n", ""), &site), 0);
        assert_eq!(known(&ran(0, "@k0 41\n", ""), &site), 5);
    }

    const OVERFLOW: &str = "sisu: panic at input.sisu:9:5: integer overflow\n";

    #[test]
    fn known_panic_is_the_last_line() {
        let site = [panics("k0", "integer overflow")];
        let stderr = format!("x\n{OVERFLOW}");
        assert_eq!(known(&ran(101, "@k0 panics\n", &stderr), &site), 0);
        let divided = "sisu: panic at input.sisu:9:5: division by zero\n";
        assert_eq!(known(&ran(101, "@k0 panics\n", divided), &site), 5);
        assert_eq!(known(&ran(0, "@k0 panics\n", ""), &site), 5);
    }

    /// A panic before a value site's line cannot hide a wrong or missing value.
    #[test]
    fn missing_known_value_fails() {
        let site = [value("k0", "42")];
        assert_eq!(known(&ran(0, "", ""), &site), 5);
        assert_eq!(known(&ran(101, "", OVERFLOW), &site), 5);
    }

    /// A panic site not reached was cut off by an earlier panic.
    #[test]
    fn unreached_panic_site_needs_a_panic() {
        let site = [panics("k0", "integer overflow")];
        assert_eq!(known(&ran(101, "1\n", OVERFLOW), &site), 0);
        assert_eq!(known(&ran(0, "1\n", ""), &site), 5);
        assert_eq!(known(&ran(101, "@k0 panics\n", OVERFLOW), &site), 0);
        assert_eq!(known(&ran(0, "@k0 panics\n", ""), &site), 5);
    }

    /// The program's own output may print `@` (`char(64)`): a value site's line is found by
    /// its place, the first lines, and its exact text, so no other line is read as one.
    #[test]
    fn other_at_lines_are_ignored() {
        let sites = [value("k0", "42"), value("k1", "7")];
        let stdout = "@k0 42\n@k1 7\n@\n@k0 41\n@k1\n";
        assert_eq!(known(&ran(0, stdout, ""), &sites), 0);
        assert_eq!(known(&ran(0, "@\n@k0 42\n@k1 7\n", ""), &sites), 5);
    }

    /// What `judge_marked` says of builds that all print `stdout`, when `shapes` should print
    /// `expected`: the failed property, 0 for none, and the failure's text.
    fn marked(stdout: &str, expected: &[&str]) -> (u8, String) {
        let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
        let runs = [ran(0, stdout, ""), ran(0, stdout, ""), ran(0, stdout, "")];
        judge_marked(&runs, &expected)
            .map_or_else(|f| (f.property, f.detail), |()| (0, String::new()))
    }

    const LIST: [&str; 3] = ["@d0 Q0.r1", "@d1 N0.r1", "@d1 Q0.r1"];

    #[test]
    fn judge_marked_should_pass_when_stdout_prints_the_whole_list() {
        assert_eq!(marked("@d0 Q0.r1\n@d1 N0.r1\n@d1 Q0.r1\n", &LIST).0, 0);
    }

    /// `main` can return or panic before `shapes`, and then no marked line prints.
    #[test]
    fn judge_marked_should_pass_when_stdout_prints_no_marked_line() {
        assert_eq!(marked("", &LIST).0, 0);
        assert_eq!(marked("@k0 7\nplain\n", &LIST).0, 0);
    }

    /// A panic inside `shapes` stops it after some marked lines.
    #[test]
    fn judge_marked_should_pass_when_stdout_prints_a_prefix_of_the_list() {
        assert_eq!(marked("@d0 Q0.r1\n@d1 N0.r1\n", &LIST).0, 0);
    }

    #[test]
    fn judge_marked_should_fail_when_a_value_is_wrong() {
        let (property, detail) = marked("@d0 Q0.r1\n@d1 Q0.r1\n", &LIST);
        assert_eq!(property, 5);
        // The first line that differs is the second, where `N0.r1` was the value to print.
        assert!(
            detail.contains("`@d1 N0.r1`") && detail.contains("`@d1 Q0.r1`"),
            "{detail}"
        );
        assert!(detail.contains("marked line 2"), "{detail}");
    }

    #[test]
    fn judge_marked_should_fail_when_two_lines_are_out_of_order() {
        let (property, detail) = marked("@d1 N0.r1\n@d0 Q0.r1\n@d1 Q0.r1\n", &LIST);
        assert_eq!(property, 5);
        assert!(detail.contains("marked line 1"), "{detail}");
    }

    #[test]
    fn judge_marked_should_fail_when_a_line_is_extra() {
        let (property, detail) = marked("@d0 Q0.r1\n@d1 N0.r1\n@d1 Q0.r1\n@d2 Q0.r1\n", &LIST);
        assert_eq!(property, 5);
        assert!(
            detail.contains("`@d2 Q0.r1`") && detail.contains("makes only 3"),
            "{detail}"
        );
    }

    /// The build that differs is the last one, `--rc-checks`: each build is judged, not the
    /// first.
    #[test]
    fn judge_marked_should_read_every_build() {
        let expected: Vec<String> = LIST.iter().map(ToString::to_string).collect();
        let right = "@d0 Q0.r1\n";
        let runs = [
            ran(0, right, ""),
            ran(0, right, ""),
            ran(0, "@d0 N0.r1\n", ""),
        ];
        let failure =
            judge_marked(&runs, &expected).expect_err("the third build prints a wrong line");
        assert!(failure.detail.contains("--rc-checks"), "{}", failure.detail);
    }

    /// The text of the item of `program` that starts with `header` at the start of a line: the
    /// line alone when it ends in `}`, else up to the line `}` that closes it.
    fn item<'a>(program: &'a str, header: &str) -> Option<&'a str> {
        let start = if program.starts_with(header) {
            0
        } else {
            program.find(&format!("\n{header}"))? + 1
        };
        let rest = &program[start..];
        if rest.lines().next()?.ends_with('}') {
            return rest.lines().next();
        }
        Some(&rest[..rest.find("\n}\n")? + 2])
    }

    /// Whether a line of `text` holds a marked statement and `part`.
    fn marks(text: &str, part: &str) -> bool {
        text.lines()
            .any(|line| line.contains("\"@d") && line.contains(part))
    }

    /// Whether a method of an interface, as `Requirement::method` writes it with a result `i64`
    /// or `string`, is in `body`.
    fn has_method(body: &str) -> bool {
        body.contains("-> i64 { ") || body.contains("-> string { ")
    }

    /// The dispatches of a call of a requirement that a program may hold, each a neighbour of
    /// #203: the call must reach the method of its own type.
    const DISPATCHES: [&str; 8] = [
        "a direct call on `Q<k>` and on `N<k>`",
        "a call through `via<k>` with `Q<k>`, `N<k>` and `R<k>`",
        "a requirement inherited from an earlier interface",
        "a result `T` picked by the expected type, `i64` and `string`",
        "`both` with two interfaces in one bound",
        "`pick<k>` with a bounded associated type",
        "a conformance in an `extend` block",
        "a conformance in the declaration list",
    ];

    /// Whether `g` holds the dispatch `row` of `DISPATCHES`, as its text shows.
    fn holds(row: &str, g: &Generated) -> bool {
        let program = g.program.as_str();
        let shapes = item(program, "fn shapes()").unwrap_or_default();
        // The most interfaces a program declares is three.
        let interfaces = 0..3;
        let conformances = |keyword: &str, prefix: &str| {
            program
                .lines()
                .filter(|line| line.starts_with(keyword) && line.contains(": I"))
                .filter_map(|line| item(program, line))
                .any(|body| has_method(body) && body.starts_with(prefix))
        };
        match row {
            "a direct call on `Q<k>` and on `N<k>`" => marks(shapes, "{q") && marks(shapes, "{n"),
            "a call through `via<k>` with `Q<k>`, `N<k>` and `R<k>`" => {
                let mut runs = BTreeMap::new();
                for line in &g.expected {
                    *runs.entry(line.split(' ').next()).or_insert(0) += 1;
                }
                runs.values().any(|&n| n >= 3)
            }
            "a requirement inherited from an earlier interface" => {
                interfaces.into_iter().any(|k| {
                    let header = item(program, &format!("interface I{k}"))
                        .and_then(|text| text.lines().next())
                        .unwrap_or_default();
                    let Some((_, list)) = header.trim_end_matches(" {").split_once(": ") else {
                        return false;
                    };
                    let via = item(program, &format!("fn via{k}<")).unwrap_or_default();
                    list.split(", ").any(|inherited| {
                        let name = inherited.split('<').next().unwrap_or_default();
                        let body = item(program, &format!("interface {name}")).unwrap_or_default();
                        (body.lines())
                            .filter_map(|line| line.trim().strip_prefix("fn "))
                            .filter_map(|line| line.split('(').next())
                            .any(|requirement| marks(via, &format!("x.{requirement}(")))
                    })
                })
            }
            "a result `T` picked by the expected type, `i64` and `string`" => {
                let mut types: BTreeMap<&str, Vec<&str>> = BTreeMap::default();
                for line in shapes
                    .lines()
                    .filter_map(|l| l.trim().strip_prefix("let d"))
                {
                    if let Some((_, rest)) = line.split_once(": ")
                        && let Some((ty, call)) = rest.split_once(" = ")
                    {
                        types
                            .entry(call.split('(').next().unwrap_or_default())
                            .or_default()
                            .push(ty);
                    }
                }
                types
                    .values()
                    .any(|types| types.contains(&"i64") && types.contains(&"string"))
            }
            "`both` with two interfaces in one bound" => {
                item(program, "fn both<").is_some_and(|body| marks(body, ""))
            }
            "`pick<k>` with a bounded associated type" => interfaces.into_iter().any(|k| {
                item(program, &format!("fn pick{k}<")).is_some_and(|body| marks(body, ""))
            }),
            "a conformance in an `extend` block" => {
                conformances("extend Q", "extend Q") || conformances("extend N", "extend N")
            }
            "a conformance in the declaration list" => {
                conformances("class Q", "class Q") || conformances("enum N", "enum N")
            }
            _ => unreachable!("{row} should be one of `DISPATCHES`"),
        }
    }

    /// The program of the first seed of the series that holds `row`, whose O0 run prints every
    /// line that `shapes` should: the seed's `Generated`, and that run.
    fn dispatch_of(row: &str) -> (Generated, Run) {
        for g in (0..coverage::SEEDS).map(|n| generate(&seed(n))) {
            if !holds(row, &g) {
                continue;
            }
            let run = o0_run(&g);
            let printed = String::from_utf8_lossy(&run.stdout)
                .lines()
                .filter(|line| line.starts_with("@d"))
                .count();
            if printed == g.expected.len() {
                return (g, run);
            }
        }
        panic!("no seed of the series holds {row} and prints its marked lines");
    }

    /// The seed's program holds the dispatch, and its O0 run prints the expected lines, which are
    /// not none.
    fn dispatch_should_print_its_expected_lines(row: &str) {
        let (g, run) = dispatch_of(row);
        assert!(!g.expected.is_empty(), "{row}");
        judge_marked(&[run], &g.expected)
            .unwrap_or_else(|f| panic!("{row}: {}\n{}", f.detail, g.program));
    }

    #[test]
    fn a_direct_call_on_a_class_and_an_enum_should_print_each_receivers_value() {
        dispatch_should_print_its_expected_lines(DISPATCHES[0]);
    }

    #[test]
    fn a_call_through_via_should_print_the_value_of_each_argument() {
        dispatch_should_print_its_expected_lines(DISPATCHES[1]);
    }

    #[test]
    fn a_requirement_from_an_inherited_interface_should_print_the_inherited_value() {
        dispatch_should_print_its_expected_lines(DISPATCHES[2]);
    }

    #[test]
    fn a_result_picked_by_its_expected_type_should_print_that_conformances_value() {
        dispatch_should_print_its_expected_lines(DISPATCHES[3]);
    }

    #[test]
    fn a_call_through_both_should_print_the_values_of_the_class_with_both() {
        dispatch_should_print_its_expected_lines(DISPATCHES[4]);
    }

    #[test]
    fn a_call_through_pick_should_print_the_value_of_the_associated_type() {
        dispatch_should_print_its_expected_lines(DISPATCHES[5]);
    }

    #[test]
    fn a_conformance_in_an_extend_block_should_print_its_own_values() {
        dispatch_should_print_its_expected_lines(DISPATCHES[6]);
    }

    #[test]
    fn a_conformance_in_the_declaration_list_should_print_its_own_values() {
        dispatch_should_print_its_expected_lines(DISPATCHES[7]);
    }

    /// Every dispatch is held by a seed of the series: the search above finds each.
    #[test]
    fn the_seeds_should_hold_every_dispatch() {
        let held: Vec<Generated> = (0..coverage::SEEDS).map(|n| generate(&seed(n))).collect();
        for row in DISPATCHES {
            assert!(held.iter().any(|g| holds(row, g)), "no seed holds {row}");
        }
    }

    /// Seed 584's `main` returns before it calls `shapes`, which the program holds, so the run
    /// prints no marked line, and the property holds. Its `return` is on line 157 of the program.
    #[test]
    fn a_main_that_returns_before_shapes_should_print_no_marked_line() {
        let g = generate(&seed(584));
        let main = item(&g.program, "fn main()").expect("the program should have a `main`");
        assert!(
            main.contains("return\n") && main.ends_with("    shapes()\n}"),
            "{main}"
        );
        assert!(
            !g.expected.is_empty(),
            "`shapes` should hold marked statements"
        );
        let run = o0_run(&g);
        assert_eq!(run.exit, Exit::Code(0), "`main` should return");
        assert!(!String::from_utf8_lossy(&run.stdout).contains("\n@d"));
        judge_marked(&[run], &g.expected).expect("a run that prints no marked line should pass");
    }

    /// `program` with the values of two methods of one requirement swapped: two methods of the
    /// same name and result type, which only `i64` and `string` results have. `None` when the
    /// program has no such two.
    fn swapped(program: &str) -> Option<String> {
        let mut lines: Vec<String> = program.lines().map(str::to_string).collect();
        let mut groups: BTreeMap<(String, String), Vec<usize>> = BTreeMap::default();
        for (at, line) in lines.iter().enumerate() {
            let Some(rest) = line.trim_start().strip_prefix("fn r") else {
                continue;
            };
            let Some((name, rest)) = rest.split_once('(') else {
                continue;
            };
            let Some((_, result)) = rest.split_once(") -> ") else {
                continue;
            };
            if let Some((ty, _)) = result.split_once(" { ")
                && name.bytes().all(|b| b.is_ascii_digit())
                && (ty == "i64" || ty == "string")
            {
                groups
                    .entry((name.to_string(), ty.to_string()))
                    .or_default()
                    .push(at);
            }
        }
        let methods = groups.values().find(|methods| methods.len() >= 2)?;
        let value = |line: &str| {
            let (_, value) = line.rsplit_once(" -> ")?.1.split_once(" { ")?;
            Some(value.trim_end_matches(" }").to_string())
        };
        let (a, b) = (methods[0], methods[1]);
        let (first, second) = (value(&lines[a])?, value(&lines[b])?);
        lines[a] = lines[a].replace(&format!("{{ {first} }}"), &format!("{{ {second} }}"));
        lines[b] = lines[b].replace(&format!("{{ {second} }}"), &format!("{{ {first} }}"));
        Some(lines.join("\n") + "\n")
    }

    /// A program of the series where two methods of one requirement, on two types, swap their
    /// values: it checks clean and exits 0, as before, and only the marked lines show it. Its
    /// call reaches the other type's value, so property 5 fails, and the failure names the first
    /// marked line that differs.
    #[test]
    fn judge_marked_should_fail_when_two_methods_swap_their_values() {
        let (g, _) = dispatch_of(DISPATCHES[0]);
        let source = swapped(&g.program).expect("the program should hold two methods of a name");
        assert_ne!(source, g.program);
        let run = on_stack(|| {
            let scratch = Scratch::new();
            let program = checked("swapped", &source).expect("a swapped program should check");
            let stdin = scratch.0.join("stdin.txt");
            fs::write(&stdin, &g.stdin).expect("the test should write the stdin");
            let exe = scratch.0.join("program");
            build("swapped", &source, &program, &BUILDS[0].1, &runtime(), &exe);
            run(&exe, &stdin, &scratch.0, LIMIT)
        });
        assert_eq!(run.exit, Exit::Code(0), "{source}");
        let failure = judge_marked(&[run], &g.expected)
            .expect_err("two swapped methods should fail property 5");
        assert_eq!(failure.property, 5);
        assert!(
            failure.detail.contains("should print `@d"),
            "{}",
            failure.detail
        );
    }

    /// The expected list differs by one value from what the program prints, as when a generator
    /// rule works out a value wrong: the property fails at that line.
    #[test]
    fn judge_marked_should_fail_when_one_expected_value_changes() {
        let (mut g, run) = dispatch_of(DISPATCHES[0]);
        let last = g.expected.len() - 1;
        g.expected[last].push('!');
        let failure = judge_marked(&[run], &g.expected).expect_err("a changed value should fail");
        assert_eq!(
            (
                failure.property,
                failure
                    .detail
                    .contains(&format!("marked line {}", last + 1))
            ),
            (5, true)
        );
    }

    /// The `n`th seed of the coverage test's series, which walks the first 64.
    fn seed(n: u64) -> Vec<u8> {
        coverage::seed(n)
    }

    /// The `n`th seed's program, or its rewritten copy, passes properties 1 to 3 through the
    /// path `judge` takes. One test per source keeps each to three builds.
    fn seed_passes(n: u64, rewritten: bool) {
        let g = generate(&seed(n));
        let (name, source) = if rewritten {
            ("rewritten", &g.rewritten)
        } else {
            ("program", &g.program)
        };
        on_stack(|| {
            let scratch = Scratch::new();
            let program = checked(name, source)
                .unwrap_or_else(|f| panic!("property {}: {}", f.property, f.detail));
            let stdin = scratch.0.join("stdin.txt");
            fs::write(&stdin, &g.stdin).expect("the test should write the stdin");
            judge_source(name, source, &program, &runtime(), &scratch.0, &stdin)
                .unwrap_or_else(|(f, record)| panic!("property 3: {}\n{record}", f.detail));
        });
    }

    /// Any seed of the series, named by `SISU_FUZZ_SEED`, as the named seed tests check theirs:
    /// `SISU_FUZZ_SEED=72 cargo nextest run -p sisuc --lib seed_from_env --run-ignored only`.
    #[test]
    #[ignore = "runs only on demand, for the seed that SISU_FUZZ_SEED names"]
    fn seed_from_env_should_pass() {
        let n = env::var("SISU_FUZZ_SEED")
            .ok()
            .and_then(|s| s.parse().ok())
            .expect("SISU_FUZZ_SEED should name a seed number");
        seed_passes(n, false);
        seed_passes(n, true);
    }

    #[test]
    fn seed_0_program_passes() {
        seed_passes(0, false);
    }

    #[test]
    fn seed_0_rewritten_passes() {
        seed_passes(0, true);
    }

    #[test]
    fn seed_1_program_passes() {
        seed_passes(1, false);
    }

    #[test]
    fn seed_1_rewritten_passes() {
        seed_passes(1, true);
    }

    #[test]
    fn seed_2_program_passes() {
        seed_passes(2, false);
    }

    #[test]
    fn seed_2_rewritten_passes() {
        seed_passes(2, true);
    }

    #[test]
    fn seed_3_program_passes() {
        seed_passes(3, false);
    }

    #[test]
    fn seed_3_rewritten_passes() {
        seed_passes(3, true);
    }

    /// Seeds 0 to 3 do not call `later`, so this seed brings its `U` and `Two` to codegen.
    #[test]
    fn seed_59_program_should_pass() {
        let g = generate(&seed(59));
        assert!(
            g.program.contains("let _ = later0("),
            "seed 59 should call `later0`"
        );
        seed_passes(59, false);
    }

    /// `program` end to end, all six builds and properties 1 to 6: it passes and leaves no
    /// directory behind. Of the seeds under 4000 with rewrites, a value site and a panic site
    /// that its run reaches, seed 3934 has the shortest program, so its builds are quick.
    #[test]
    fn program_should_pass_and_leave_no_directory_behind() {
        let tmp = Scratch::new();
        let data = seed(3934);
        let g = generate(&data);
        assert!(
            g.rewritten != g.program && panic_sites(&g) == 1 && g.known.len() > 1,
            "seed 3934 should have rewrites, a value site and a panic site"
        );
        on_stack(|| program_in(&data, &runtime(), &tmp.0));
        let left = fs::read_dir(&tmp.0)
            .expect("the test's directory should be readable")
            .count();
        assert_eq!(left, 0, "`program` should remove its directory");
    }

    /// Seed 3934's one panic site is the last thing its O0 run prints, as the marker `item.rs`
    /// writes. `judge_known` takes a run without that line for an earlier panic, so with it
    /// property 5 judged this panic site.
    #[test]
    fn a_seed_with_a_panic_site_should_print_its_marker_last() {
        let g = generate(&seed(3934));
        assert_eq!(panic_sites(&g), 1, "seed 3934 should have one panic site");
        let marker = g
            .known
            .iter()
            .find_map(|known| matches!(known.answer, Answer::Panic(_)).then_some(&known.marker))
            .expect("seed 3934 should have a panic site");
        let Run { exit, stdout, .. } = o0_run(&g);
        assert_eq!(exit, Exit::Code(101), "{}", g.program);
        assert_eq!(
            String::from_utf8_lossy(&stdout).lines().last(),
            Some(format!("@{marker} panics").as_str()),
            "{}",
            g.program
        );
    }

    /// Seed 2950's program runs `dbg` eight times, and a rewrite moves some of its lines. The
    /// lines of the two runs differ as written. Property 4 holds when `unlocated` takes the
    /// `<file>:<line>:<col>: ` off each. The text and the value of each line are the same, since
    /// no rewrite changes the argument of a `dbg`.
    #[test]
    fn a_seed_with_dbg_should_keep_property_4_across_its_rewrites() {
        let g = generate(&seed(2950));
        let copy = Generated {
            program: g.rewritten.clone(),
            ..generate(&seed(2950))
        };
        let (program, rewritten) = (o0_run(&g), o0_run(&copy));
        let lines = program
            .stderr
            .split(|&b| b == b'\n')
            .filter(|line| dbg_location(line) > 0)
            .count();
        assert_eq!(lines, 8, "{}", g.program);
        assert_ne!(
            program.stderr, rewritten.stderr,
            "a rewrite should move a line"
        );
        assert_eq!(pair(&program, &rewritten), 0);
    }

    /// A program with one bound site: the argument at `at`, which `broken` replaces.
    fn with_site(program: &str, at: Range<usize>, broken: &'static str) -> Generated {
        Generated {
            program: program.to_string(),
            rewritten: String::new(),
            stdin: String::new(),
            known: Vec::new(),
            expected: Vec::new(),
            bound_sites: vec![BoundSite {
                at,
                bound: "Eq".to_string(),
                broken,
            }],
            cost: 0,
            plain: String::new(),
            made: Vec::new(),
        }
    }

    /// `fn f<T: Eq>` called with `1`, in a program of `tail` more lines of `main`, and its site.
    fn eq_site(tail: &str, broken: &'static str) -> Generated {
        let program =
            format!("fn f<T: Eq>(x: T) -> i64 {{ 1 }}\nfn main() {{\n    let _ = f(1)\n{tail}}}\n");
        let at = program.find("(1)").expect("the program should call `f`") + 1;
        with_site(&program, at..at + 1, broken)
    }

    #[test]
    fn property_6_should_hold_when_only_the_site_breaks_its_bound() {
        judge_bounds(&eq_site("", "NoBound()")).expect("one E0154 at the argument should pass");
    }

    #[test]
    fn property_6_should_fail_when_the_replacement_conforms() {
        let failure = judge_bounds(&eq_site("", "2")).expect_err("no error should fail");
        assert_eq!(failure.property, 6);
        assert!(
            failure.detail.contains("reports no error"),
            "{}",
            failure.detail
        );
        // The line and column of the argument, the replaced text and the bound.
        assert!(
            failure
                .detail
                .contains("`2` replaces `1` at 3:15, to break the bound `Eq`"),
            "{}",
            failure.detail
        );
        assert_eq!(
            failure.broken.as_deref(),
            Some(
                "fn f<T: Eq>(x: T) -> i64 { 1 }\nfn main() {\n    let _ = f(2)\n}\nclass NoBound {}\n"
            )
        );
    }

    #[test]
    fn property_6_should_fail_when_another_error_follows_the_one_at_the_site() {
        let failure = judge_bounds(&eq_site("    let _: i64 = \"s\"\n", "NoBound()"))
            .expect_err("a second error elsewhere should fail");
        assert_eq!(failure.property, 6);
        assert!(
            failure.detail.contains("not E0154 at the argument"),
            "{}",
            failure.detail
        );
        // The diagnostics are in the detail, E0154 as well as the other one.
        assert!(failure.detail.contains("E0154"), "{}", failure.detail);
        assert!(failure.detail.contains("E0032"), "{}", failure.detail);
    }

    #[test]
    fn property_6_should_fail_when_the_error_at_the_site_is_not_e0154() {
        let failure = judge_bounds(&eq_site("", "nothing"))
            .expect_err("an unknown name at the site should fail");
        assert_eq!(failure.property, 6);
        assert!(
            failure.detail.contains("not E0154 at the argument"),
            "{}",
            failure.detail
        );
    }

    #[test]
    fn property_6_should_fail_when_another_e0154_is_not_at_the_site() {
        // The second call breaks the bound as well, so the checker reports E0154 twice.
        let failure = judge_bounds(&eq_site("    let _ = f(NoBound())\n", "NoBound()"))
            .expect_err("an E0154 at another argument should fail");
        assert_eq!(failure.property, 6);
        assert!(
            failure.detail.contains("not E0154 at the argument"),
            "{}",
            failure.detail
        );
    }

    /// Property 6 for every bound site of the seeds `from..from + 2`, which the replacement of that
    /// site must break and nothing else. Two seeds for each test, so that each stays within its
    /// time budget (the slowest takes 0.8 s alone) and nextest runs them in parallel. The 32 tests
    /// together take the 64 seeds that `coverage::seeds` gives.
    fn seeds_should_report_only_their_bound(from: u64) {
        const _: () = assert!(coverage::SEEDS == 64, "add or drop a test of two seeds");
        for n in from..from + 2 {
            judge_bounds(&generate(&seed(n)))
                .unwrap_or_else(|f| panic!("seed {n}: property {}: {}", f.property, f.detail));
        }
    }

    #[test]
    fn the_bound_sites_of_seeds_0_and_1_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(0);
    }

    #[test]
    fn the_bound_sites_of_seeds_2_and_3_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(2);
    }

    #[test]
    fn the_bound_sites_of_seeds_4_and_5_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(4);
    }

    #[test]
    fn the_bound_sites_of_seeds_6_and_7_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(6);
    }

    #[test]
    fn the_bound_sites_of_seeds_8_and_9_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(8);
    }

    #[test]
    fn the_bound_sites_of_seeds_10_and_11_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(10);
    }

    #[test]
    fn the_bound_sites_of_seeds_12_and_13_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(12);
    }

    #[test]
    fn the_bound_sites_of_seeds_14_and_15_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(14);
    }

    #[test]
    fn the_bound_sites_of_seeds_16_and_17_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(16);
    }

    #[test]
    fn the_bound_sites_of_seeds_18_and_19_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(18);
    }

    #[test]
    fn the_bound_sites_of_seeds_20_and_21_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(20);
    }

    #[test]
    fn the_bound_sites_of_seeds_22_and_23_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(22);
    }

    #[test]
    fn the_bound_sites_of_seeds_24_and_25_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(24);
    }

    #[test]
    fn the_bound_sites_of_seeds_26_and_27_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(26);
    }

    #[test]
    fn the_bound_sites_of_seeds_28_and_29_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(28);
    }

    #[test]
    fn the_bound_sites_of_seeds_30_and_31_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(30);
    }

    #[test]
    fn the_bound_sites_of_seeds_32_and_33_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(32);
    }

    #[test]
    fn the_bound_sites_of_seeds_34_and_35_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(34);
    }

    #[test]
    fn the_bound_sites_of_seeds_36_and_37_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(36);
    }

    #[test]
    fn the_bound_sites_of_seeds_38_and_39_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(38);
    }

    #[test]
    fn the_bound_sites_of_seeds_40_and_41_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(40);
    }

    #[test]
    fn the_bound_sites_of_seeds_42_and_43_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(42);
    }

    #[test]
    fn the_bound_sites_of_seeds_44_and_45_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(44);
    }

    #[test]
    fn the_bound_sites_of_seeds_46_and_47_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(46);
    }

    #[test]
    fn the_bound_sites_of_seeds_48_and_49_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(48);
    }

    #[test]
    fn the_bound_sites_of_seeds_50_and_51_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(50);
    }

    #[test]
    fn the_bound_sites_of_seeds_52_and_53_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(52);
    }

    #[test]
    fn the_bound_sites_of_seeds_54_and_55_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(54);
    }

    #[test]
    fn the_bound_sites_of_seeds_56_and_57_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(56);
    }

    #[test]
    fn the_bound_sites_of_seeds_58_and_59_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(58);
    }

    #[test]
    fn the_bound_sites_of_seeds_60_and_61_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(60);
    }

    #[test]
    fn the_bound_sites_of_seeds_62_and_63_should_report_only_their_bound() {
        seeds_should_report_only_their_bound(62);
    }

    /// How many of `g`'s known answers are panics.
    fn panic_sites(g: &Generated) -> usize {
        g.known
            .iter()
            .filter(|known| matches!(known.answer, Answer::Panic(_)))
            .count()
    }

    /// `g`'s program checked, built at O0 and run on its stdin.
    fn o0_run(g: &Generated) -> Run {
        on_stack(|| {
            let scratch = Scratch::new();
            let program = checked("program", &g.program).expect("a seed should check clean");
            let stdin = scratch.0.join("stdin.txt");
            fs::write(&stdin, &g.stdin).expect("the test should write the stdin");
            let exe = scratch.0.join("program");
            build(
                "program",
                &g.program,
                &program,
                &BUILDS[0].1,
                &runtime(),
                &exe,
            );
            run(&exe, &stdin, &scratch.0, LIMIT)
        })
    }

    /// Loops nested as deep as decision 5 lets them, each run to its bound, finish at O0 within
    /// the limit, so a bound written past what the cost counts shows as `TimedOut`. No
    /// assertion on time.
    #[test]
    fn a_budget_program_should_finish_within_the_limit() {
        // Byte by byte: no types, no generic types, one instance of `Result` over two `i64`s,
        // no generic functions, no items, a statement
        // of `main`, a `while` of bound 8 and no loop label, twice more inside it, and in the
        // innermost a `print` of a literal; then every choice is the first.
        let g = generate(&[
            0, 0, 0, 0, 0, 0, 0, 1, 7, 8, 0, 0, 1, 7, 8, 0, 0, 1, 7, 8, 0, 0, 1, 0, 0, 0, 0,
        ]);
        let Run { exit, stdout, .. } = o0_run(&g);
        assert_eq!(exit, Exit::Code(0), "{}", g.program);
        // One line from each pass of the innermost loop: every loop ran to its bound. The six
        // more lines are the two that `lang_both` prints, the three that `nest` prints and the
        // one that prints its result.
        assert_eq!(
            String::from_utf8_lossy(&stdout).lines().count(),
            8 * 8 * 8 + 6
        );
    }

    /// Set in a child process that one of the tests below starts from this binary.
    const CHILD: &str = "SISU_FUZZ_PROGRAM_CHILD";

    /// Runs this binary's test `name` alone as a child process, with `TMPDIR` the empty
    /// `scratch/tmp` and `SISU_FUZZ_DUMP` `scratch/dump`.
    fn child(name: &str, scratch: &Path) -> Output {
        let tmp = scratch.join("tmp");
        fs::create_dir(&tmp).expect("the test should make the child's temporary directory");
        Command::new(env::current_exe().expect("the test binary should know its path"))
            .args([
                "--exact",
                &format!("fuzz_program::tests::{name}"),
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("TMPDIR", tmp)
            .env("SISU_FUZZ_DUMP", scratch.join("dump"))
            .output()
            .expect("the test binary should run again")
    }

    /// Asserts that `stderr` holds `first`, then each of `g`'s sources under its `==>` line, and
    /// that the child left nothing in its temporary directory.
    fn assert_record(stderr: &str, first: &str, g: &Generated, scratch: &Path) {
        let at = |text: &str| {
            stderr
                .find(text)
                .unwrap_or_else(|| panic!("stderr should hold {text:?}:\n{stderr}"))
        };
        let sections = [
            at(first),
            at(&format!("==> program\n{}", g.program)),
            at(&format!("==> rewritten\n{}", g.rewritten)),
            at(&format!("==> stdin\n{}", g.stdin)),
        ];
        assert!(sections.is_sorted(), "{sections:?}:\n{stderr}");
        let left = fs::read_dir(scratch.join("tmp"))
            .expect("the child's temporary directory should be readable")
            .count();
        assert_eq!(left, 0, "the child should remove its directory");
    }

    #[test]
    fn a_failure_should_print_the_program() {
        if env::var_os(CHILD).is_some() {
            on_stack(|| {
                FORCE.set(Some(Force::Stdout));
                program(&[], &runtime());
            });
            return;
        }
        let scratch = Scratch::new();
        let out = child("a_failure_should_print_the_program", &scratch.0);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{stderr}");
        let g = generate(&[]);
        assert_record(&stderr, "property 3: ", &g, &scratch.0);
        let builds = [
            stderr.find("==> stdin\n"),
            // Output without a final line break gets one. The empty input's program prints the
            // two lines of `lang_both`, the three of `nest` and the one that prints its result.
            stderr.find("==> program -O2 stdout\nforced\n==> program -O2 stderr\n"),
            stderr.find(
                "==> program --rc-checks stdout\nRanked\n[1, 2]\nA(A(End))\ntrue\nfalse\n2\n==> program --rc-checks stderr\n",
            ),
            stderr.find("program fuzzer: property 3 failed"),
        ];
        assert!(
            builds.iter().all(Option::is_some) && builds.is_sorted(),
            "{builds:?}:\n{stderr}"
        );
        let failure = fs::read_to_string(scratch.0.join("dump/failure.txt"))
            .expect("the dump should hold failure.txt");
        assert!(failure.starts_with("property 3: "), "{failure}");
    }

    /// libFuzzer's panic hook aborts, so the record must be written before the earlier hook runs.
    #[test]
    fn a_compiler_panic_leaves_the_dump() {
        if env::var_os(CHILD).is_some() {
            std::panic::set_hook(Box::new(|_| std::process::abort()));
            on_stack(|| {
                FORCE.set(Some(Force::CompilerPanic));
                program(&seed(0), &runtime());
            });
            return;
        }
        let scratch = Scratch::new();
        let out = child("a_compiler_panic_leaves_the_dump", &scratch.0);
        let stderr = String::from_utf8_lossy(&out.stderr);
        // SIGABRT is 6 on Linux and macOS.
        assert_eq!(out.status.signal(), Some(6), "{stderr}");
        let g = generate(&seed(0));
        assert_record(&stderr, "compiler panic\n==> program\n", &g, &scratch.0);
        let dump = scratch.0.join("dump");
        let read = |name: &str| {
            fs::read_to_string(dump.join(name))
                .unwrap_or_else(|e| panic!("the dump should hold {name}: {e}"))
        };
        assert_eq!(read("program.sisu"), g.program);
        assert_eq!(read("rewritten.sisu"), g.rewritten);
        assert_eq!(read("stdin.txt"), g.stdin);
        let failure = read("failure.txt");
        assert!(failure.starts_with("compiler panic\n"), "{failure}");
        assert!(
            failure.contains("a compiler panic forced by a test"),
            "{failure}"
        );
        assert!(failure.contains("fuzz_program.rs:"), "{failure}");
    }

    /// A dump that cannot be written is reported, and the record still reaches stderr.
    #[test]
    fn a_dump_it_cannot_write_is_reported() {
        if env::var_os(CHILD).is_some() {
            on_stack(|| {
                FORCE.set(Some(Force::CompilerPanic));
                program(&seed(0), &runtime());
            });
            return;
        }
        let scratch = Scratch::new();
        // A file where the dump's directory should be.
        fs::write(scratch.0.join("dump"), "").expect("the test should write the file");
        let out = child("a_dump_it_cannot_write_is_reported", &scratch.0);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{stderr}");
        let g = generate(&seed(0));
        assert_record(&stderr, "compiler panic\n==> program\n", &g, &scratch.0);
        assert!(stderr.contains("cannot write "), "{stderr}");
    }

    /// The first seed of the series whose program has bound sites. A new generator rule changes
    /// what each seed makes, so the test finds the seed and does not name it.
    fn bound_seed() -> u64 {
        (0..64)
            .find(|&n| !generate(&seed(n)).bound_sites.is_empty())
            .expect("a seed of the first 64 should have a bound site")
    }

    /// A failed property 6 leaves the broken copy in the dump, and no build ran. The forced
    /// replacement of the first bound site of `bound_seed` conforms, so the property fails.
    #[test]
    fn a_failed_bound_should_leave_the_broken_copy_in_the_dump() {
        if env::var_os(CHILD).is_some() {
            on_stack(|| {
                FORCE.set(Some(Force::Conforming));
                program(&seed(bound_seed()), &runtime());
            });
            return;
        }
        let scratch = Scratch::new();
        let out = child(
            "a_failed_bound_should_leave_the_broken_copy_in_the_dump",
            &scratch.0,
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{stderr}");
        let g = generate(&seed(bound_seed()));
        assert_record(&stderr, "property 6: ", &g, &scratch.0);
        let dump = scratch.0.join("dump");
        let read = |name: &str| {
            fs::read_to_string(dump.join(name))
                .unwrap_or_else(|e| panic!("the dump should hold {name}: {e}"))
        };
        // The program as it is, since the forced replacement conforms.
        assert_eq!(
            read("broken.sisu"),
            format!("{}class NoBound {{}}\n", g.program)
        );
        let failure = read("failure.txt");
        assert!(failure.starts_with("property 6: "), "{failure}");
        assert_eq!(read("program.sisu"), g.program);
    }
}
