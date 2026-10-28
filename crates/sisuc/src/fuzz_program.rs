//! The entry point of the program fuzz target: generates a Sisu program from the input bytes,
//! builds it and its rewritten copy at O0, at `-O2` and with `--rc-checks`, runs each build and
//! panics on a breach of a property (ADR 0033). Properties 1 and 2 are that the program checks
//! clean and compiles to a module LLVM verifies; property 3 is that every build exits 0 or 101,
//! and all three the same way; property 4 is that the rewritten copy runs as the program does;
//! property 5 is that the program prints the known integer answers.

use std::borrow::Cow;
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
use crate::generate::{self, Answer, Generated, Known};
use crate::report::{Format, Reporter};
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
}

/// The input `program` is judging on this thread, for the panic hook.
struct Current {
    generated: Generated,
    /// The input's temporary directory.
    dir: PathBuf,
    /// Set by a property failure before its panic: the property line and the failed builds.
    failure: Option<(String, String)>,
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
            current
                .as_mut()
                .expect("`program` should have set the input")
                .failure = Some((format!("property {property}: {}", failure.detail), builds));
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
            [
                ("program.sisu", program.as_str()),
                ("rewritten.sisu", rewritten.as_str()),
                ("stdin.txt", stdin.as_str()),
                ("failure.txt", failure.as_str()),
            ]
            .into_iter()
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

/// Properties 1 to 5 for `g`, built in `dir`. A failure comes with the record of the builds
/// that showed it.
fn judge(g: &Generated, runtime: &Path, dir: &Path) -> Result<(), (Failure, String)> {
    let program = checked("program", &g.program).map_err(|f| (f, String::new()))?;
    let rewritten = checked("rewritten", &g.rewritten).map_err(|f| (f, String::new()))?;
    let stdin = dir.join("stdin.txt");
    fs::write(&stdin, &g.stdin).expect("the fuzzer should write the stdin to its own directory");
    let runs = judge_source("program", &g.program, &program, runtime, dir, &stdin)?;
    let copies = judge_source("rewritten", &g.rewritten, &rewritten, runtime, dir, &stdin)?;
    judge_pair(&runs[0], &copies[0])
        .map_err(|f| (f, record("program", &runs) + &record("rewritten", &copies)))?;
    judge_known(&runs[0], &g.known).map_err(|f| (f, record("program", &runs)))
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

/// Properties 1 and 2: `source` checks with no error, W0010 or W0011, and its MIR certifies and its
/// module verifies, without and with `--rc-checks` (`fuzz::assert_compiles` panics if not).
fn checked(name: &str, source: &str) -> Result<Program, Failure> {
    let (program, diagnostics) = front_end::front_end(source);
    let failure = |what: String| {
        let mut out = Vec::new();
        let mut reporter = Reporter::new(&mut out, Format::Text, false);
        reporter.diagnostics(&format!("{name}.sisu"), source, &diagnostics);
        reporter.finish();
        Failure {
            property: 1,
            detail: format!("{what}\n{}", String::from_utf8_lossy(&out)),
        }
    };
    let Some(program) = program else {
        return Err(failure(format!("the checker rejects the {name}")));
    };
    // Every generated function returns and every generated loop ends, so none may call itself
    // on every path, and no loop condition may stay unchanged.
    if let Some(d) = diagnostics
        .iter()
        .find(|d| matches!(d.code().as_str(), "W0010" | "W0011"))
    {
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
    let fail = |detail: String| {
        Err(Failure {
            property: 3,
            detail,
        })
    };
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
        Some((what, _)) => Err(Failure {
            property: 4,
            detail: format!("the program and its rewritten copy differ in {what}"),
        }),
        None => Ok(()),
    }
}

/// The prefix of the runtime's panic line, before its `<file>:<line>:<col>: <message>`.
const PANIC: &str = "sisu: panic at ";

/// `run`'s stderr, with the `<file>:<line>:<col>: ` of a panic's line taken out: only from the
/// last line, and only when the run exited 101, so any other line compares as written.
fn unlocated(run: &Run) -> Cow<'_, [u8]> {
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
    match location {
        Some(end) if run.exit == Exit::Code(101) => {
            Cow::Owned([&stderr[..after], &stderr[after + end + 2..]].concat())
        }
        _ => Cow::Borrowed(stderr),
    }
}

/// Property 5: `run`, the program's O0 run, prints each known value site's line as the first
/// lines of its stdout, in order; and a panic site, when its `@k<n> panics` line is the last,
/// panics with the expected message, while a run that does not reach it panics before it.
/// Only those exact lines are read, so another line that starts with `@` changes nothing.
fn judge_known(run: &Run, known: &[Known]) -> Result<(), Failure> {
    let fail = |detail: String| {
        Err(Failure {
            property: 5,
            detail,
        })
    };
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
}

#[cfg(test)]
thread_local! {
    static FORCE: std::cell::Cell<Option<Force>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::ExitStatusExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::time::Duration;
    use std::{env, fs, thread};

    use super::{
        Answer, BUILDS, Exit, FORCE, Force, Known, LIMIT, Run, build, builds, checked, judge,
        judge_builds, judge_known, judge_pair, judge_source, program, program_in, run,
    };
    use crate::fresh_names;
    use crate::generate::{Generated, coverage, generate};

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

    /// `target/debug/libsisu_runtime_mimalloc.a`, the default allocator's runtime, which
    /// nextest's setup script builds: the test binary is `target/debug/deps/<name>` (decision 7).
    fn runtime() -> PathBuf {
        let exe = env::current_exe().expect("the test binary should know its path");
        exe.parent()
            .and_then(Path::parent)
            .expect("the test binary should be two levels inside the target directory")
            .join("libsisu_runtime_mimalloc.a")
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
    fn a_rejected_program_fails_property_1() {
        let clean = "fn main() {\n    print(1)\n}\n";
        let rejected = "fn main() {\n    print(x)\n}\n";
        for (program, rewritten) in [(rejected, clean), (clean, rejected)] {
            let g = Generated {
                program: program.to_string(),
                rewritten: rewritten.to_string(),
                stdin: String::new(),
                known: Vec::new(),
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

    /// `program` end to end, all six builds and properties 1 to 5: it passes and leaves no
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
        // One line from each pass of the innermost loop: every loop ran to its bound.
        assert_eq!(String::from_utf8_lossy(&stdout).lines().count(), 8 * 8 * 8);
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
    fn a_failure_prints_the_program() {
        if env::var_os(CHILD).is_some() {
            on_stack(|| {
                FORCE.set(Some(Force::Stdout));
                program(&[], &runtime());
            });
            return;
        }
        let scratch = Scratch::new();
        let out = child("a_failure_prints_the_program", &scratch.0);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{stderr}");
        let g = generate(&[]);
        assert_record(&stderr, "property 3: ", &g, &scratch.0);
        let builds = [
            stderr.find("==> stdin\n"),
            // Output without a final line break gets one; the empty input's program, an empty
            // `main`, prints nothing, which gets none.
            stderr.find("==> program -O2 stdout\nforced\n==> program -O2 stderr\n"),
            stderr.find("==> program --rc-checks stdout\n==> program --rc-checks stderr\n"),
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
}
