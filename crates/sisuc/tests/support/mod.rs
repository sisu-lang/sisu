//! Helpers shared by the end-to-end test binaries.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;

pub(crate) mod selfhost;

/// `sisu build <flags> <input> -o <output>`, run from `dir`, after asserting both runtime archives
/// exist.
pub(crate) fn sisu(dir: &Path, input: &str, output: &Path, flags: &[&str]) -> Command {
    let sisu = Path::new(env!("CARGO_BIN_EXE_sisu"));
    assert!(
        ["libsisu_runtime_mimalloc.a", "libsisu_runtime_system.a"]
            .iter()
            .all(|archive| sisu.with_file_name(archive).exists()),
        "libsisu_runtime_mimalloc.a or libsisu_runtime_system.a is missing: run the tests with \
         `cargo nextest run`, whose setup script builds both, or run `cargo build --workspace`"
    );
    let mut command = Command::new(sisu);
    command
        .current_dir(dir)
        .arg("build")
        .args(flags)
        .arg(input)
        .arg("-o")
        .arg(output);
    command
}

/// Compiles `<dir>/<name>.sisu`, run from `dir`, into `exe`, passing `flags` to `sisu`, and
/// returns `sisu`'s stderr, which holds its warnings.
pub(crate) fn compile(dir: &Path, name: &str, exe: &Path, flags: &[&str]) -> Vec<u8> {
    let out = sisu(dir, &format!("{name}.sisu"), exe, flags)
        .output()
        .expect("sisu starts");
    assert_eq!(
        out.status.code(),
        Some(0),
        "sisu failed on {name}.sisu:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stderr
}

/// The unused-name warnings that no case may get. A case can keep other warnings.
const UNUSED: [&str; 3] = ["warning[W0012]", "warning[W0013]", "warning[W0014]"];

/// Whether `stderr` holds an `UNUSED` warning: a line that starts with one, as a diagnostic's
/// first line does. A source line in an excerpt starts with its line number, so it does not count.
pub(crate) fn has_unused_warning(stderr: &str) -> bool {
    stderr
        .lines()
        .any(|line| UNUSED.iter().any(|code| line.starts_with(code)))
}

/// The marker line that gives a case's arguments: `// args: <arg> <arg> ...`.
const ARGS: &str = "// args:";

/// What a case's programs run with: its standard input and its arguments.
#[derive(Default)]
pub(crate) struct Invocation {
    stdin: Vec<u8>,
    args: Vec<OsString>,
}

impl Invocation {
    /// The invocation of the case `path`: its standard input is `<name>.in`, or empty when the
    /// case has none. Its arguments are those of its `ARGS` line, split on ASCII spaces with
    /// empty pieces dropped, so an argument cannot hold a space; none without the line. A case
    /// has one such line at most.
    pub(crate) fn of(path: &Path) -> Invocation {
        let in_path = path.with_extension("in");
        let stdin = match fs::read(&in_path) {
            Ok(stdin) => stdin,
            Err(error) if error.kind() == ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!("{}: {error}", in_path.display()),
        };
        let source = fs::read_to_string(path).expect("reads the case");
        let mut markers = source.lines().filter_map(|line| line.strip_prefix(ARGS));
        let args = markers.next().map_or_else(Vec::new, |marker| {
            marker
                .split(' ')
                .filter(|arg| !arg.is_empty())
                .map(OsString::from)
                .collect()
        });
        assert!(
            markers.next().is_none(),
            "{}: more than one `{ARGS}` line",
            path.display()
        );
        Invocation::new(stdin, args)
    }

    /// An invocation with `stdin` as its standard input and `args` as its arguments.
    pub(crate) fn new(stdin: Vec<u8>, args: Vec<OsString>) -> Invocation {
        Invocation { stdin, args }
    }

    /// Runs `exe` with this standard input and these arguments; with `valgrind` on Linux, under
    /// Valgrind, where any leak or memory error makes it exit 1. Elsewhere Valgrind does not
    /// run, so `exe` runs natively and the Linux jobs alone check for leaks. On Linux it refuses
    /// a build that links mimalloc, before Valgrind starts (see `refuse_mimalloc`).
    pub(crate) fn run(&self, exe: &Path, valgrind: bool) -> Output {
        if valgrind && cfg!(target_os = "linux") {
            return self.run_under(exe, Path::new("valgrind"));
        }
        let mut command = Command::new(exe);
        command.args(&self.args);
        output_with_stdin(&mut command, &self.stdin)
    }

    /// Runs `exe` with `ASAN_OPTIONS` set to [`ASAN_OPTIONS`], which replaces any value in the
    /// environment, and without `LSAN_OPTIONS`. The environment of the developer then cannot
    /// turn off a report.
    pub(crate) fn run_asan(&self, exe: &Path) -> Output {
        let mut command = Command::new(exe);
        command
            .env("ASAN_OPTIONS", ASAN_OPTIONS)
            .env_remove("LSAN_OPTIONS")
            .args(&self.args);
        output_with_stdin(&mut command, &self.stdin)
    }

    /// Runs `exe` under the Valgrind program `valgrind`, with the three flags of `run`. It refuses
    /// a build that links mimalloc, before `valgrind` starts (see `refuse_mimalloc`).
    pub(crate) fn run_under(&self, exe: &Path, valgrind: &Path) -> Output {
        refuse_mimalloc(exe);
        let mut command = Command::new(valgrind);
        command
            .args([
                "--leak-check=full",
                "--errors-for-leak-kinds=definite,indirect,possible",
                "--error-exitcode=1",
            ])
            .arg(exe)
            .args(&self.args);
        output_with_stdin(&mut command, &self.stdin)
    }
}

/// `sisu`'s flags for a build that runs under Valgrind: on Linux, the runtime on the System
/// allocator, since Valgrind sees no block that mimalloc hands out. Valgrind runs nowhere else.
pub(crate) const VALGRIND_FLAGS: &[&str] = if cfg!(target_os = "linux") {
    &["--allocator", "system"]
} else {
    &[]
};

/// The names that `nm --defined-only` lists for `exe`. It reads symbols, not bytes, so a program
/// whose text holds a name does not list it; a stripped build, or a failed `nm`, stops the run.
/// `sisu_alloc` must be among them, or the allocator that `exe` links is unknown.
pub(crate) fn defined_symbols(exe: &Path) -> Vec<String> {
    let out = Command::new("nm")
        .arg("--defined-only")
        .arg(exe)
        .output()
        .expect("nm starts");
    assert!(
        out.status.success(),
        "`nm --defined-only {}` failed: {:?}\n{}",
        exe.display(),
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let listing = String::from_utf8_lossy(&out.stdout);
    let names: Vec<String> = listing
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .map(str::to_owned)
        .collect();
    assert!(
        names.iter().any(|name| name == "sisu_alloc"),
        "{}: `nm` lists no `sisu_alloc`, so the allocator it links is unknown",
        exe.display()
    );
    names
}

/// Whether `names` lists `mi_malloc_aligned`, the function mimalloc's global allocator calls.
fn links_mimalloc(names: &[String]) -> bool {
    names.iter().any(|name| name == "mi_malloc_aligned")
}

/// Panics when `exe` links mimalloc, whose blocks Valgrind cannot see.
pub(crate) fn refuse_mimalloc(exe: &Path) {
    assert!(
        !links_mimalloc(&defined_symbols(exe)),
        "{} links mimalloc, whose blocks Valgrind cannot see: build it with `--allocator system`",
        exe.display()
    );
}

/// Panics unless `exe` links mimalloc: a build with no `--allocator` flag must. Call it on Linux
/// only: a macOS symbol name has a leading `_`, so `links_mimalloc` finds none there.
pub(crate) fn require_mimalloc(exe: &Path) {
    assert!(
        links_mimalloc(&defined_symbols(exe)),
        "{} does not link mimalloc, which a build with no `--allocator` flag links",
        exe.display()
    );
}

/// Runs `command` with `stdin` written to its standard input, and collects its output. A
/// thread writes the input while this one reads the output, so neither pipe can fill up and
/// stall the other. A program that exits before it reads all its input makes the write fail
/// with `BrokenPipe`, which is ignored.
fn output_with_stdin(command: &mut Command, stdin: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| {
            let program = command.get_program();
            let hint = if program == "valgrind" {
                " (Valgrind installs with `apt install valgrind`)"
            } else {
                ""
            };
            panic!("{} does not start{hint}: {error}", program.display())
        });
    let mut pipe = child.stdin.take().expect("stdin should be piped");
    thread::scope(|scope| {
        // The pipe drops when the write ends, so the program sees the end of its input.
        let writer = scope.spawn(move || {
            drop(pipe.write_all(stdin));
        });
        let output = child.wait_with_output().expect("the program should run");
        writer.join().expect("the stdin writer should not panic");
        output
    })
}

/// The output test of a `tests/run` case: a plain build and an `-O2` build, both on the default
/// allocator, mimalloc, differ, exit 0 and agree on stdout and on stderr. That stdout is then
/// `<name>.out`. It builds into `case_dir`. The checks run in this order:
/// 1. On Linux, both builds link mimalloc. On macOS a symbol name has a leading `_`, so this check
///    does not run there.
/// 2. The `-O2` build is not identical to the plain build.
/// 3. Each `// o2-calls:` and `// o2-indirect-calls:` marker in the case holds in the `-O2` IR (see
///    `O2_CALLS` and `O2_INDIRECT_CALLS`).
/// 4. The plain build runs, then the `-O2` build. Each exits 0. Its stderr is empty, unless
///    `<name>.err` exists.
/// 5. The two builds agree on stdout and on stderr.
/// 6. Stdout is `<name>.out`, and stderr is `<name>.err` when that file exists. With `bless`,
///    rewrites those files instead. This step is last, so a failed check writes nothing.
///
/// Every run reads `<name>.in` on its standard input, or empty input without one, and gets the
/// arguments of the case's `// args:` line. `memory` runs the memory checkers on the same case.
pub(crate) fn run(path: &Path, bless: bool) {
    let [o0, o2] = build(path, [O0, MIMALLOC_O2]);
    if cfg!(target_os = "linux") {
        require_mimalloc(&o0);
        require_mimalloc(&o2);
    }
    assert!(
        fs::read(&o0).expect("reads the plain build")
            != fs::read(&o2).expect("reads the -O2 build"),
        "{}: -O2 build is identical to the plain build",
        path.display()
    );
    o2_calls(path);
    o2_indirect_calls(path);
    let invocation = Invocation::of(path);
    let err_path = path.with_extension("err");
    let expects_stderr = err_path.exists();
    let [out0, out2] = [&o0, &o2].map(|exe| {
        let out = invocation.run(exe, false);
        assert_exits_zero(exe, &out);
        if !expects_stderr {
            assert_eq!(
                String::from_utf8_lossy(&out.stderr),
                "",
                "{}: writes to stderr, and the case has no `.err` file",
                exe.display()
            );
        }
        out
    });
    let stdout = agreed(path, "stdout", &out0.stdout, &out2.stdout);
    let stderr = agreed(path, "stderr", &out0.stderr, &out2.stderr);
    let mut files = vec![(path.with_extension("out"), stdout)];
    if expects_stderr {
        files.push((err_path, stderr));
    }
    expect_files(&files, bless);
}

/// The memory test of a `tests/run` case. Each run exits 0, prints `<name>.out` on stdout, and
/// prints `<name>.err` on stderr, or nothing when that file does not exist.
///
/// On Linux, the checks run in this order:
/// 1. Three builds that `ASan` checks run: `ASAN_O0`, `ASAN_O2` and `ASAN_RC`. The `asan-O0`
///    build is the first program that runs, so that a stack overflow shows as an `ASan` report and
///    not as a signal.
/// 2. The `-O2` build on the System allocator (`VALGRIND_O2`) runs natively.
/// 3. That build runs once under Valgrind, which finds no leak or memory error. Its stderr holds
///    Valgrind's own log, so it is not compared.
///
/// On macOS, `ASan` and Valgrind do not run. A `--rc-checks` build runs, which finds no count
/// fault or live object at exit.
///
/// Every run reads `<name>.in` and gets the arguments of the `// args:` line, as in `run`. It
/// builds into `memory_dir`, so it can run at the same time as `run` on the same case. It has no
/// bless mode and writes no file.
pub(crate) fn memory(path: &Path) {
    memory_with(path, Path::new("valgrind"));
}

/// `memory`, with `valgrind` as the Valgrind program.
pub(crate) fn memory_with(path: &Path, valgrind: &Path) {
    let out_path = path.with_extension("out");
    let stdout = read_if_exists(&out_path).unwrap_or_else(|| {
        panic!(
            "{} is missing; create it empty, then bless the output test with SISU_BLESS=1",
            out_path.display()
        )
    });
    let stderr = read_if_exists(&path.with_extension("err")).unwrap_or_default();
    let invocation = Invocation::of(path);
    let check = |exe: &Path, out: &Output| {
        assert_exits_zero(exe, out);
        assert_same_streams(exe, out, &stdout, &stderr);
    };
    let out_dir = memory_dir(path);
    if !cfg!(target_os = "linux") {
        let [rc] = build_into(path, &out_dir, [RC_CHECKS]);
        check(&rc, &invocation.run(&rc, false));
        return;
    }
    let [asan_o0, asan_o2, asan_rc, o2] =
        build_into(path, &out_dir, [ASAN_O0, ASAN_O2, ASAN_RC, VALGRIND_O2]);
    // `asan-O0` first, see above.
    for exe in [&asan_o0, &asan_o2, &asan_rc] {
        check(exe, &invocation.run_asan(exe));
    }
    check(&o2, &invocation.run(&o2, false));
    let checked = invocation.run_under(&o2, valgrind);
    let valgrind_stderr = String::from_utf8_lossy(&checked.stderr);
    assert_eq!(
        checked.status.code(),
        Some(0),
        "{}: {valgrind_stderr}",
        o2.display()
    );
    // shortcut: Valgrind 3.26 runs arm64 FMINNM and FMAXNM as FMIN and FMAX
    // (VEX/priv/guest_arm64_toIR.c:32), so `min` or `max` of a `nan` prints `nan` under it;
    // compare stdout on aarch64 again once a Valgrind release fixes that.
    if !cfg!(target_arch = "aarch64") {
        assert_eq!(
            String::from_utf8_lossy(&checked.stdout),
            stdout,
            "{}: {valgrind_stderr}",
            o2.display()
        );
    }
}

/// The text of `path`, or `None` when the file does not exist.
fn read_if_exists(path: &Path) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => panic!("{}: {error}", path.display()),
    }
}

/// Panics unless `out`, the output of `exe`, shows exit code 0.
fn assert_exits_zero(exe: &Path, out: &Output) {
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}: {}\n{}",
        exe.display(),
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Panics unless `out`, the output of `exe`, holds `stdout` on stdout and `stderr` on stderr.
fn assert_same_streams(exe: &Path, out: &Output, stdout: &str, stderr: &str) {
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        stderr,
        "{}",
        exe.display()
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        stdout,
        "{}",
        exe.display()
    );
}

/// Checks a `tests/run-fail` case: both builds exit 101, agree, and print `<name>.out` on stdout
/// and `<name>.err` on stderr, with the standard input and arguments of a `tests/run` case. With
/// `bless`, rewrites both files instead. No Valgrind: a panic exits without releasing.
pub(crate) fn run_fail(path: &Path, bless: bool) {
    let invocation = Invocation::of(path);
    let [out0, out2] = build(path, [O0, O2]).map(|exe| {
        let out = invocation.run(&exe, false);
        assert_eq!(
            out.status.code(),
            Some(101),
            "{}: {:?}\n{}",
            exe.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        out
    });
    let stdout = agreed(path, "stdout", &out0.stdout, &out2.stdout);
    let stderr = agreed(path, "stderr", &out0.stderr, &out2.stderr);
    expect_files(
        &[
            (path.with_extension("out"), stdout),
            (path.with_extension("err"), stderr),
        ],
        bless,
    );
}

/// Checks a `tests/diagnostics` case; see `diagnose`.
pub(crate) fn diagnostics(path: &Path, bless: bool) {
    diagnose(path, path, bless);
}

/// The codes that only `mono` reports. `sisu check` does not run `mono`, so `diagnose` runs
/// `sisu build --emit mir` for a case of these codes. Add each new code of `mono` to this list.
const MONO_CODES: [&str; 1] = ["E0163"];

/// The arguments of `sisu` that report the diagnostics of a case of `code`, before `--format`
/// and the file.
fn report_args(code: &str) -> &'static [&'static str] {
    if MONO_CODES.contains(&code) {
        &["build", "--emit", "mir"]
    } else {
        &["check"]
    }
}

/// Checks that `sisu check <name>.sisu` on `source` (`sisu build --emit mir` for a code in
/// `MONO_CODES`), run from its directory, reports at least
/// one diagnostic, prints nothing on stdout, and exits 1 when a stderr line starts with `error`,
/// else 0. A source named after a code (`E0001.sisu`, `E0001-field.sisu`) reports that code.
/// `sisu check --format json` exits the same, and each of its stderr lines is a JSON object,
/// the summary last. Each `line:column` in the comment lines at the top of `source` is where
/// a primary or a secondary span of the JSON starts. Then the two stderrs are `expected` with the
/// extensions `err` and `json`; with `bless`, rewrites both files instead.
pub(crate) fn diagnose(source: &Path, expected: &Path, bless: bool) {
    let dir = source.parent().expect("a case should sit in a directory");
    let name = source.file_name().expect("a case should have a file name");
    let sisu = Path::new(env!("CARGO_BIN_EXE_sisu"));
    let stem = source
        .file_stem()
        .and_then(OsStr::to_str)
        .expect("a case's name should be UTF-8");
    let code = stem.split_once('-').map_or(stem, |(code, _)| code);
    let args = report_args(code);
    let out = Command::new(sisu)
        .current_dir(dir)
        .args(args)
        .arg(name)
        .output()
        .expect("sisu starts");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let display = source.display();
    assert!(!stderr.is_empty(), "{display} reports no diagnostic");
    let failed = stderr.lines().any(|l| l.starts_with("error"));
    assert_eq!(
        out.status.code(),
        Some(i32::from(failed)),
        "{display}: {stderr}"
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "", "{display}");
    if is_code(code) {
        assert!(
            stderr.contains(&format!("[{code}]")),
            "{display} does not report {code}: {stderr}"
        );
    }
    let out = Command::new(sisu)
        .current_dir(dir)
        .args(args)
        .args(["--format", "json"])
        .arg(name)
        .output()
        .expect("sisu starts");
    let json = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(
        out.status.code(),
        Some(i32::from(failed)),
        "{display} --format json: {json}"
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "", "{display}");
    assert!(
        json.lines()
            .all(|l| l.starts_with(r#"{"version":1,"kind":""#)),
        "{display}: a stderr line is not a JSON object: {json}"
    );
    assert!(
        json.lines()
            .last()
            .is_some_and(|l| l.contains(r#""kind":"summary""#)),
        "{display}: the summary is not last: {json}"
    );
    let bytes = fs::read(source).expect("a case should be readable");
    for (line, column) in header_positions(&String::from_utf8_lossy(&bytes)) {
        assert!(
            reports_at(&json, line, column),
            "{display}: the header names {line}:{column}, but no primary or secondary span \
             starts there"
        );
    }
    expect_files(
        &[
            (expected.with_extension("err"), stderr),
            (expected.with_extension("json"), json),
        ],
        bless,
    );
}

/// Each `line:column` in the comment lines at the top of `source`, such as the `7:9` of
/// `// THEN it reports E0038 at 7:9` or the `(6:10)` of `// THEN it reports E0011 (6:10)`. A word
/// that holds a letter, such as `E0038:3`, is not a position.
pub(crate) fn header_positions(source: &str) -> Vec<(u64, u64)> {
    let number = |text: &str| -> Option<u64> {
        (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse().ok())
            .flatten()
    };
    source
        .lines()
        .take_while(|line| line.starts_with("//"))
        .flat_map(|line| line.split(|c: char| !c.is_ascii_alphanumeric() && c != ':'))
        .filter_map(|word| {
            let (line, column) = word.split_once(':')?;
            Some((number(line)?, number(column)?))
        })
        .collect()
}

/// Whether a diagnostic of `json`, one JSON object a line, has a primary or a secondary span
/// that starts at `line` and `column`.
pub(crate) fn reports_at(json: &str, line: u64, column: u64) -> bool {
    let at = |span: &serde_json::Value| span["line"] == line && span["column"] == column;
    json.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .any(|d| {
            at(&d["primary"])
                || d["secondary"]
                    .as_array()
                    .is_some_and(|spans| spans.iter().any(at))
        })
}

/// Whether `text` is a diagnostic code: `E` or `W`, then four digits.
pub(crate) fn is_code(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 5 && matches!(bytes[0], b'E' | b'W') && bytes[1..].iter().all(u8::is_ascii_digit)
}

/// The extensions of the expectation files that a run-mode case directory may hold.
pub(crate) const RUN_EXPECTATIONS: [&str; 3] = ["out", "err", "in"];

/// Lists `dir` once and returns one message for each entry that no case reads: a hidden entry
/// (a stray `.ghost.out`), and any file that is neither a `.sisu` case nor a file whose
/// extension is one of `expectations` (the files the directory's mode reads) beside a `.sisu`
/// of the same name. A case directory is flat, so a subdirectory is one more bad entry. The
/// messages come in name order. Panics when `dir` holds no `.sisu`, as a wrong `dir` would
/// otherwise pass with no entry checked.
pub(crate) fn orphans(dir: &Path, expectations: &[&str]) -> Vec<String> {
    let paths = sorted_entries(dir);
    assert!(
        paths
            .iter()
            .any(|path| path.extension().is_some_and(|ext| ext == "sisu")),
        "{} holds no `.sisu` case",
        dir.display()
    );
    paths
        .iter()
        .filter_map(|path| {
            stray_message(path).or_else(|| {
                let belongs = match path.extension().and_then(OsStr::to_str) {
                    Some("sisu") => true,
                    Some(ext) if expectations.contains(&ext) => {
                        path.with_extension("sisu").exists()
                    }
                    _ => false,
                };
                (!belongs).then(|| {
                    format!(
                        "{} is neither a `.sisu` case nor the {expectations:?} file of one",
                        path.display()
                    )
                })
            })
        })
        .collect()
}

/// The paths in `dir`, sorted, so that a message list does not depend on the file system's order.
pub(crate) fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .expect("the case directory should be readable")
        .map(|entry| entry.expect("a directory entry should be readable").path())
        .collect();
    paths.sort();
    paths
}

/// The message for a hidden or directory `path`, or `None` for any other. The harness never
/// lists a hidden file, so a stray `.ghost.out` would otherwise go unseen.
pub(crate) fn stray_message(path: &Path) -> Option<String> {
    let display = path.display();
    if path
        .file_name()
        .is_some_and(|name| name.as_encoded_bytes().starts_with(b"."))
    {
        Some(format!(
            "{display} is hidden, so no case reads it; remove it"
        ))
    } else if path.is_dir() {
        Some(format!(
            "{display} is a directory; this directory holds only files"
        ))
    } else {
        None
    }
}

/// Writes each `(path, contents)` under a fresh `<CARGO_TARGET_TMPDIR>/fixtures/<test>/` and
/// returns that directory.
pub(crate) fn fixture(test: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("fixtures")
        .join(test);
    // It is absent on the first run, so a failure here is expected and harmless.
    drop(fs::remove_dir_all(&dir));
    for (path, contents) in files {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().expect("a fixture file should have a parent"))
            .expect("creates the fixture directory");
        fs::write(path, contents).expect("writes the fixture file");
    }
    dir
}

/// The builds `build` makes: the executable's name in the case directory, and `sisu`'s flags.
pub(crate) type Build = (&'static str, &'static [&'static str]);
/// The `ASAN_OPTIONS` of each run of a sanitized build. `fast_unwind_on_malloc=0` gives a report
/// whose stack holds the frames of the program. GCC's `libasan` before GCC 13 does not check a use
/// after return unless `detect_stack_use_after_return=1` is set.
pub(crate) const ASAN_OPTIONS: &str = "fast_unwind_on_malloc=0:detect_stack_use_after_return=1";

pub(crate) const ASAN_O0: Build = ("asan-O0", &["--sanitize", "address"]);
pub(crate) const ASAN_O2: Build = ("asan-O2", &["--sanitize", "address", "-O2"]);
pub(crate) const ASAN_RC: Build = ("asan-rc", &["--sanitize", "address", "--rc-checks"]);
const O0: Build = ("O0", &[]);
const O2: Build = ("O2", &["-O2"]);
pub(crate) const RC_CHECKS: Build = ("rc", &["--rc-checks"]);
/// `-O2` on the default allocator, mimalloc, the allocator that users get: the `-O2` build of
/// `run`. On Linux, `memory` builds `VALGRIND_O2` in its place, a System build, for Valgrind.
pub(crate) const MIMALLOC_O2: Build = ("O2-mimalloc", &["-O2"]);
/// `--rc-checks` on the System allocator, for a test that runs a count-checked build under
/// Valgrind on Linux, as it runs `VALGRIND_O0`. `memory` does not build it: there, `ASAN_RC` checks
/// the counted build. Its stderr is Valgrind's own log, and is not compared. On macOS Valgrind
/// does not run, and `RC_CHECKS` is this build already.
pub(crate) const VALGRIND_RC: Build = (
    "rc-valgrind",
    if cfg!(target_os = "linux") {
        &["--rc-checks", "--allocator", "system"]
    } else {
        &["--rc-checks"]
    },
);
/// `O0` and `O2` for a test that runs a plain build under Valgrind on Linux, on the System
/// allocator. `memory` builds `VALGRIND_O2` and runs it once under Valgrind, but it builds no plain
/// build on the System allocator.
pub(crate) const VALGRIND_O0: Build = ("O0", VALGRIND_FLAGS);
// Repeats `VALGRIND_FLAGS`' values after `-O2`, and must stay in step with them: a const cannot
// concatenate slices.
pub(crate) const VALGRIND_O2: Build = (
    "O2",
    if cfg!(target_os = "linux") {
        &["-O2", "--allocator", "system"]
    } else {
        &["-O2"]
    },
);

/// Compiles `path` once for each of `builds` into its `case_dir`, and returns the executables.
/// A build with an `UNUSED` warning fails the case.
fn build<const N: usize>(path: &Path, builds: [Build; N]) -> [PathBuf; N] {
    build_into(path, &case_dir(path), builds)
}

/// `build`, into the existing directory `out`.
fn build_into<const N: usize>(path: &Path, out: &Path, builds: [Build; N]) -> [PathBuf; N] {
    let dir = path.parent().expect("a case should sit in a directory");
    let name = path
        .file_stem()
        .and_then(OsStr::to_str)
        .expect("a case's name should be UTF-8");
    builds.map(|(file, flags)| {
        let exe = out.join(file);
        let stderr = compile(dir, name, &exe, flags);
        let stderr = String::from_utf8_lossy(&stderr);
        assert!(
            !has_unused_warning(&stderr),
            "{}: an unused name, which the case must not have:\n{stderr}",
            path.display()
        );
        exe
    })
}

/// `<CARGO_TARGET_TMPDIR>/cases/<source path>/`, where `case_dir` puts the builds of `source`. The
/// whole path, nested, keeps same-named cases in different directories apart.
pub(crate) fn case_path(source: &Path) -> PathBuf {
    build_path("cases", source)
}

/// `<CARGO_TARGET_TMPDIR>/memory-cases/<source path>/`, where `memory_dir` puts the builds of
/// `source`. `run` and `memory` of one case run at the same time, so their builds stay apart.
pub(crate) fn memory_path(source: &Path) -> PathBuf {
    build_path("memory-cases", source)
}

/// `<CARGO_TARGET_TMPDIR>/<root>/<source path>/`.
fn build_path(root: &str, source: &Path) -> PathBuf {
    // An absolute path would replace the base in `join`. A leading `..`, as a case outside the
    // crate has, would climb out of `root`. So both go in without those components.
    let inside: PathBuf = source
        .components()
        .skip_while(|c| matches!(c, Component::RootDir | Component::ParentDir))
        .collect();
    Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(root)
        .join(inside)
}

/// A fresh, empty `case_path`.
pub(crate) fn case_dir(source: &Path) -> PathBuf {
    fresh_dir(case_path(source))
}

/// A fresh, empty `memory_path`.
fn memory_dir(source: &Path) -> PathBuf {
    fresh_dir(memory_path(source))
}

/// `dir`, emptied, or made when it does not exist.
fn fresh_dir(dir: PathBuf) -> PathBuf {
    // It is absent on the first run, so a failure here is expected and harmless.
    drop(fs::remove_dir_all(&dir));
    fs::create_dir_all(&dir).expect("creates the case directory");
    dir
}

/// The marker a `tests/run` case puts on its own line to prove a call boundary: with
/// `// o2-calls: <caller> <callee> <count>`, the `-O2` IR of `<caller>` holds exactly `<count>`
/// calls to `<callee>`, so the inliner kept the calls the case relies on. A count of 0 is
/// rejected, because it holds when every call is gone. Every function but
/// the C `main` is internal, so `-O2` would inline a caller with one call site and the caller
/// would vanish from the IR. The check therefore builds once per distinct caller, with
/// `SISU_KEEP_FUNCTIONS` set to that caller alone, which marks it `noinline` for
/// `sisu build --emit ir` and nothing else, and counts in that build. One caller at a time, so
/// that keeping one caller does not also keep a callee that another marker names as its caller. A
/// caller that takes parameters is rejected: kept as a function of its own, it could hold a
/// call that its real call sites decide away. `main(args)` is the exception: only the C `main`
/// calls it, with arguments known at run time.
const O2_CALLS: &str = "// o2-calls:";

/// The marker a `tests/run` case puts on its own line to prove a call through a table:
/// with `// o2-indirect-calls: <function> <count>`, the body of `<function>` in the `-O2` IR
/// holds exactly `<count>` calls through a pointer, so the case keeps the call that it relies
/// on. The count holds every call through a pointer in the body, the release of an owned `any`
/// too. A count of 0 is rejected, because it holds when every such call is gone. A call through
/// a table has no symbol to name, so `O2_CALLS` cannot count it.
/// The check builds the case once with `sisu build -O2 --emit ir` and no `SISU_KEEP_FUNCTIONS`,
/// so the body is the real one, and no rule on parameters is needed. `<function>` must still be a
/// function of its own after the pipeline. It is not when `-O2` inlined it, and its body is then
/// not its own, or when `-O2` made a clone of it (`<function>.specialized.<n>`), because the
/// original can keep its call while the clones run without it. Make `<function>` recurse, and
/// not to a depth that its call sites fix, so that `-O2` keeps it.
const O2_INDIRECT_CALLS: &str = "// o2-indirect-calls:";

/// Whether the IR `line` holds a call through a pointer: a `call` whose target is a `%` value.
/// `call` is a whole word: not after `%`, `@`, `.`, `-`, `$` or a word character, and followed by
/// a space, so `%call = tail call` matches at the second `call` only, and `br label %call` not at
/// all. The target is the first token that starts with `%` or `@` and holds `(`, so a return
/// type such as `{ i64, i1 }`, a named type, `fastcc` and `range(i64 0, 6)` are skipped. A
/// target in a quoted name that holds a space is not found. A line of a function body holds no
/// string, so a `call` in a string is not a case here.
pub(crate) fn is_indirect_call(line: &str) -> bool {
    line.match_indices("call ").any(|(at, word)| {
        let after_a_name = line[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || "%@._-$".contains(c));
        !after_a_name
            && line[at + word.len()..]
                .split_whitespace()
                .find(|token| token.starts_with(['%', '@']) && token.contains('('))
                .is_some_and(|target| target.starts_with('%'))
    })
}

/// Whether `ir` defines a clone of `function` that the optimizer made:
/// `@sisu.<function>.specialized.<n>`, plain or quoted.
pub(crate) fn has_specialized_clone(ir: &str, function: &str) -> bool {
    let clone = format!("sisu.{function}.specialized.");
    let (plain, quoted) = (format!("@{clone}"), format!("@\"{clone}"));
    ir.lines()
        .filter(|line| line.starts_with("define "))
        .any(|line| line.contains(&plain) || line.contains(&quoted))
}

/// How LLVM's text names the function `name`: `@sisu.name`, or `@"sisu.name"` when the name holds
/// a character outside an identifier's, as an instantiation's `<` and `,` are.
pub(crate) fn symbol(name: &str) -> String {
    let symbol = format!("sisu.{name}");
    if symbol
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-$._".contains(c))
    {
        format!("@{symbol}")
    } else {
        format!("@\"{symbol}\"")
    }
}

/// The text of the function `name` in `ir`, from its header line to its closing brace, if `ir`
/// defines it.
pub(crate) fn function_text<'a>(ir: &'a str, name: &str) -> Option<&'a str> {
    let header = format!("{}(", symbol(name));
    ir.split("\ndefine ")
        .skip(1)
        .find(|function| function.lines().next().is_some_and(|l| l.contains(&header)))
        .and_then(|function| function.split("\n}").next())
}

/// `sisu build -O2 --emit ir` on the case `path` with `SISU_KEEP_FUNCTIONS=caller`, as the module
/// before and the module after the pipeline.
fn o2_ir(path: &Path, caller: Option<&str>) -> (String, String) {
    let dir = path.parent().expect("a case should sit in a directory");
    let name = path.file_name().expect("a case should have a file name");
    let mut command = Command::new(env!("CARGO_BIN_EXE_sisu"));
    command
        .current_dir(dir)
        .args(["build", "-O2", "--emit", "ir"])
        .arg(name);
    // With no caller, a value set in the test run's own environment must not keep a function.
    match caller {
        Some(caller) => command.env("SISU_KEEP_FUNCTIONS", caller),
        None => command.env_remove("SISU_KEEP_FUNCTIONS"),
    };
    let out = command.output().expect("sisu starts");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    let (before, after) = ir
        .split_once("\n; after default<O2>\n")
        .unwrap_or_else(|| panic!("{}: no `; after default<O2>` line", path.display()));
    (before.to_string(), after.to_string())
}

/// The words of each line of `source` that starts with `marker`, where `shape` names the `N`
/// words that a marker line holds.
fn marker_words<'a, const N: usize>(
    path: &Path,
    source: &'a str,
    marker: &str,
    shape: &str,
) -> Vec<[&'a str; N]> {
    source
        .lines()
        .filter_map(|line| line.trim().strip_prefix(marker))
        .map(|words| {
            words
                .split_whitespace()
                .collect::<Vec<_>>()
                .try_into()
                .unwrap_or_else(|_| {
                    panic!("{}: `{marker}{words}` is not `{shape}`", path.display())
                })
        })
        .collect()
}

/// Checks each `O2_CALLS` marker in `path` against `sisu build -O2 --emit ir`, built once per
/// caller.
/// The marked caller is measured as a standalone function. In a real build it is inlined, and a
/// call to a callee that LLVM infers as `memory(none)` or `readonly` can then be merged with an
/// identical call at the call site. So mark calls to callees that write memory.
fn o2_calls(path: &Path) {
    let source = fs::read_to_string(path).expect("reads the case");
    let markers: Vec<[&str; 3]> =
        marker_words(path, &source, O2_CALLS, "<caller> <callee> <count>");
    let mut built: Vec<(&str, (String, String))> = Vec::new();
    for [outer, callee, count] in markers {
        let count: usize = count.parse().unwrap_or_else(|_| {
            panic!(
                "{}: `{O2_CALLS} {outer} {callee} {count}` count is not a number",
                path.display()
            )
        });
        // A count of 0 holds when the optimizer removed every call, so it proves no call boundary.
        assert!(
            count > 0,
            "{}: `{O2_CALLS} {outer} {callee} 0` proves no call boundary; change the case so \
             -O2 keeps a call, and count it",
            path.display()
        );
        if !built.iter().any(|(caller, _)| *caller == outer) {
            built.push((outer, o2_ir(path, Some(outer))));
        }
        let (_, (before, optimized)) = built
            .iter()
            .find(|(caller, _)| *caller == outer)
            .expect("the caller was just built");
        for function in [outer, callee] {
            assert!(
                function_text(before, function).is_some(),
                "{}: no function named `{function}`",
                path.display()
            );
        }
        let header = function_text(before, outer)
            .and_then(|text| text.lines().next())
            .expect("the caller is in the IR");
        assert!(
            header.contains(&format!("{}()", symbol(outer)))
                || (outer == "main" && header.contains("@sisu.main(ptr")),
            "{}: o2-calls caller `{outer}` takes parameters; its call sites can decide the \
             call, so mark a caller without parameters",
            path.display()
        );
        let body = function_text(optimized, outer)
            .unwrap_or_else(|| panic!("{}: no `{outer}` in the -O2 IR", path.display()));
        let target = format!("{}(", symbol(callee));
        let kept = body
            .lines()
            .filter(|line| line.contains("call ") && line.contains(&target))
            .count();
        assert_eq!(
            kept,
            count,
            "{}: -O2 keeps {kept} calls to `{callee}` in `{outer}`, not {count}; make `{callee}` \
             recurse, and not to a depth the call sites fix, so the optimizer keeps it\n{body}",
            path.display()
        );
    }
}

/// Checks each `O2_INDIRECT_CALLS` marker in `path` against one `sisu build -O2 --emit ir`, built
/// only when the case has a marker.
fn o2_indirect_calls(path: &Path) {
    let source = fs::read_to_string(path).expect("reads the case");
    let markers: Vec<[&str; 2]> =
        marker_words(path, &source, O2_INDIRECT_CALLS, "<function> <count>");
    if markers.is_empty() {
        return;
    }
    let (before, optimized) = o2_ir(path, None);
    for [function, count] in markers {
        let count: usize = count.parse().unwrap_or_else(|_| {
            panic!(
                "{}: `{O2_INDIRECT_CALLS} {function} {count}` count is not a number",
                path.display()
            )
        });
        // A count of 0 holds when the optimizer removed every call, so it proves no call.
        assert!(
            count > 0,
            "{}: `{O2_INDIRECT_CALLS} {function} 0` proves no indirect call; change the case so \
             -O2 keeps one, and count it",
            path.display()
        );
        assert!(
            function_text(&before, function).is_some(),
            "{}: no function named `{function}`",
            path.display()
        );
        let body = function_text(&optimized, function)
            .filter(|_| !has_specialized_clone(&optimized, function))
            .unwrap_or_else(|| {
                panic!(
                    "{}: -O2 inlined or specialized `{function}`, so its body is not its own; \
                     make it recurse, and not to a depth its call sites fix",
                    path.display()
                )
            });
        let kept = body.lines().filter(|line| is_indirect_call(line)).count();
        assert_eq!(
            kept,
            count,
            "{}: -O2 keeps {kept} indirect calls in `{function}`, not {count}\n{body}",
            path.display()
        );
    }
}

/// `o0`, which must equal `o2`, as text; `what` names the stream in the failure.
fn agreed(path: &Path, what: &str, o0: &[u8], o2: &[u8]) -> String {
    let (o0, o2) = (String::from_utf8_lossy(o0), String::from_utf8_lossy(o2));
    assert!(
        o0 == o2,
        "{}: O0 and -O2 disagree on {what}\nO0:\n{o0}\n-O2:\n{o2}",
        path.display()
    );
    o0.into_owned()
}

/// Checks that each `(path, actual)` file holds `actual`. With `bless`, writes each `actual` to
/// its file instead, but only if every file exists: bless never creates an expectation file, and
/// writes all of them or none.
pub(crate) fn expect_files(files: &[(PathBuf, String)], bless: bool) {
    if bless {
        for (path, _) in files {
            assert!(
                path.exists(),
                "{} is missing; create it empty, then bless again",
                path.display()
            );
        }
        for (path, actual) in files {
            fs::write(path, actual).expect("writes the expectation file");
        }
    } else {
        for (path, actual) in files {
            let display = path.display();
            let expected = fs::read_to_string(path).unwrap_or_else(|error| {
                panic!("{display}: {error}; create it empty, then run with SISU_BLESS=1")
            });
            assert_eq!(
                *actual, expected,
                "{display} does not match; run with SISU_BLESS=1 to rewrite it"
            );
        }
    }
}
