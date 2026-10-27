//! Helpers shared by the end-to-end test binaries.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;

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
        let mut command = if valgrind && cfg!(target_os = "linux") {
            refuse_mimalloc(exe);
            let mut command = Command::new("valgrind");
            command
                .args([
                    "--leak-check=full",
                    "--errors-for-leak-kinds=definite,indirect,possible",
                    "--error-exitcode=1",
                ])
                .arg(exe);
            command
        } else {
            Command::new(exe)
        };
        command.args(&self.args);
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

/// Panics unless `nm --defined-only` lists `exe`'s symbols, `sisu_alloc` among them, and no
/// `mi_malloc_aligned`, the function mimalloc's global allocator calls. It reads symbols, not
/// bytes, so a program whose text holds that name passes; a stripped build, or a failed `nm`,
/// stops the run rather than passing it.
fn refuse_mimalloc(exe: &Path) {
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
    let names: Vec<&str> = listing
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .collect();
    assert!(
        names.contains(&"sisu_alloc"),
        "{}: `nm` lists no `sisu_alloc`, so the allocator it links is unknown",
        exe.display()
    );
    assert!(
        !names.contains(&"mi_malloc_aligned"),
        "{} links mimalloc, whose blocks Valgrind cannot see: build it with `--allocator system`",
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

/// Checks a `tests/run` case: both builds differ, exit 0, agree on stdout and on stderr, and
/// print the same under Valgrind on Linux, which finds no leak or memory error, and in a
/// `--rc-checks` build, which finds no count fault or live object at exit; then that stdout is
/// `<name>.out`.
/// Stderr is empty, unless `<name>.err` exists: then the stderr of the plain, `-O2` and
/// `--rc-checks` runs is that file. The Valgrind runs' stderr holds Valgrind's own log, so it is
/// never compared.
/// Every run reads `<name>.in` on its standard input, or empty input without one, and gets the
/// arguments of the case's `// args:` line.
/// Each `// o2-calls:` marker in the case must hold in the `-O2` IR (see `O2_CALLS`).
/// With `bless`, rewrites `<name>.out`, and `<name>.err` when it exists, instead, after every
/// other check passed.
pub(crate) fn run(path: &Path, bless: bool) {
    let [o0, o2, rc] = build(path, [VALGRIND_O0, VALGRIND_O2, RC_CHECKS]);
    assert!(
        fs::read(&o0).expect("reads the plain build")
            != fs::read(&o2).expect("reads the -O2 build"),
        "{}: -O2 build is identical to the plain build",
        path.display()
    );
    o2_calls(path);
    let invocation = Invocation::of(path);
    let err_path = path.with_extension("err");
    let expects_stderr = err_path.exists();
    let [out0, out2] = [&o0, &o2].map(|exe| {
        let out = invocation.run(exe, false);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}: {:?}\n{stderr}",
            exe.display(),
            out.status
        );
        if !expects_stderr {
            assert_eq!(
                stderr,
                "",
                "{}: writes to stderr, and the case has no `.err` file",
                exe.display()
            );
        }
        out
    });
    let stdout = agreed(path, "stdout", &out0.stdout, &out2.stdout);
    let stderr = agreed(path, "stderr", &out0.stderr, &out2.stderr);
    // macOS has no Valgrind, and the native runs above already checked these builds.
    if cfg!(target_os = "linux") {
        for exe in [o0, o2] {
            let checked = invocation.run(&exe, true);
            let valgrind_stderr = String::from_utf8_lossy(&checked.stderr);
            assert_eq!(
                checked.status.code(),
                Some(0),
                "{}: {valgrind_stderr}",
                exe.display()
            );
            // shortcut: Valgrind 3.26 runs arm64 FMINNM and FMAXNM as FMIN and FMAX
            // (VEX/priv/guest_arm64_toIR.c:32), so `min` or `max` of a `nan` prints `nan` under it;
            // compare stdout on aarch64 again once a Valgrind release fixes that.
            if !cfg!(target_arch = "aarch64") {
                assert_eq!(
                    String::from_utf8_lossy(&checked.stdout),
                    stdout,
                    "{}: {valgrind_stderr}",
                    exe.display()
                );
            }
        }
    }
    let checked = invocation.run(&rc, false);
    let rc_stderr = String::from_utf8_lossy(&checked.stderr);
    assert_eq!(
        checked.status.code(),
        Some(0),
        "{}: {:?}\n{rc_stderr}",
        rc.display(),
        checked.status
    );
    assert_eq!(rc_stderr, stderr, "{}", rc.display());
    assert_eq!(
        String::from_utf8_lossy(&checked.stdout),
        stdout,
        "{}",
        rc.display()
    );
    // Last, so that a run failing any check above writes nothing.
    let mut files = vec![(path.with_extension("out"), stdout)];
    if expects_stderr {
        files.push((err_path, stderr));
    }
    expect_files(&files, bless);
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

/// Checks that `sisu check <name>.sisu` on `source`, run from its directory, reports at least
/// one diagnostic, prints nothing on stdout, and exits 1 when a stderr line starts with `error`,
/// else 0. A source named after a code (`E0001.sisu`, `E0001-field.sisu`) reports that code.
/// `sisu check --format json` exits the same, and each of its stderr lines is a JSON object,
/// the summary last. Then the two stderrs are `expected` with the extensions `err` and `json`;
/// with `bless`, rewrites both files instead.
pub(crate) fn diagnose(source: &Path, expected: &Path, bless: bool) {
    let dir = source.parent().expect("a case should sit in a directory");
    let name = source.file_name().expect("a case should have a file name");
    let sisu = Path::new(env!("CARGO_BIN_EXE_sisu"));
    let out = Command::new(sisu)
        .current_dir(dir)
        .arg("check")
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
    let stem = source
        .file_stem()
        .and_then(OsStr::to_str)
        .expect("a case's name should be UTF-8");
    let code = stem.split_once('-').map_or(stem, |(code, _)| code);
    if is_code(code) {
        assert!(
            stderr.contains(&format!("[{code}]")),
            "{display} does not report {code}: {stderr}"
        );
    }
    let out = Command::new(sisu)
        .current_dir(dir)
        .args(["check", "--format", "json"])
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
    expect_files(
        &[
            (expected.with_extension("err"), stderr),
            (expected.with_extension("json"), json),
        ],
        bless,
    );
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
type Build = (&'static str, &'static [&'static str]);
const O0: Build = ("O0", &[]);
const O2: Build = ("O2", &["-O2"]);
const RC_CHECKS: Build = ("rc", &["--rc-checks"]);
/// `O0` and `O2` for `run`, whose plain builds also run under Valgrind.
const VALGRIND_O0: Build = ("O0", VALGRIND_FLAGS);
// Repeats `VALGRIND_FLAGS`' values after `-O2`, and must stay in step with them: a const cannot
// concatenate slices.
const VALGRIND_O2: Build = (
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
    let dir = path.parent().expect("a case should sit in a directory");
    let name = path
        .file_stem()
        .and_then(OsStr::to_str)
        .expect("a case's name should be UTF-8");
    let out = case_dir(path);
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

/// A fresh, empty `<CARGO_TARGET_TMPDIR>/cases/<source path>/`. The whole path, nested, keeps
/// same-named cases in different directories apart.
pub(crate) fn case_dir(source: &Path) -> PathBuf {
    // An absolute path would replace the base in `join`, and a leading `..`, as a case outside
    // the crate has, would climb out of `cases/`, so both go in without those components.
    let inside: PathBuf = source
        .components()
        .skip_while(|c| matches!(c, Component::RootDir | Component::ParentDir))
        .collect();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cases")
        .join(inside);
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

/// How LLVM's text names the function `name`: `@sisu.name`, or `@"sisu.name"` when the name holds
/// a character outside an identifier's, as an instantiation's `<` and `,` are.
fn symbol(name: &str) -> String {
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
fn function_text<'a>(ir: &'a str, name: &str) -> Option<&'a str> {
    let header = format!("{}(", symbol(name));
    ir.split("\ndefine ")
        .skip(1)
        .find(|function| function.lines().next().is_some_and(|l| l.contains(&header)))
        .and_then(|function| function.split("\n}").next())
}

/// `sisu build -O2 --emit ir` on the case `path` with `SISU_KEEP_FUNCTIONS=caller`, as the module
/// before and the module after the pipeline.
fn o2_ir(path: &Path, caller: &str) -> (String, String) {
    let dir = path.parent().expect("a case should sit in a directory");
    let name = path.file_name().expect("a case should have a file name");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .env("SISU_KEEP_FUNCTIONS", caller)
        .args(["build", "-O2", "--emit", "ir"])
        .arg(name)
        .output()
        .expect("sisu starts");
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

/// Checks each `O2_CALLS` marker in `path` against `sisu build -O2 --emit ir`, built once per
/// caller.
/// The marked caller is measured as a standalone function. In a real build it is inlined, and a
/// call to a callee that LLVM infers as `memory(none)` or `readonly` can then be merged with an
/// identical call at the call site. So mark calls to callees that write memory.
fn o2_calls(path: &Path) {
    let source = fs::read_to_string(path).expect("reads the case");
    let markers: Vec<[&str; 3]> = source
        .lines()
        .filter_map(|line| line.trim().strip_prefix(O2_CALLS))
        .map(|marker| {
            marker
                .split_whitespace()
                .collect::<Vec<_>>()
                .try_into()
                .unwrap_or_else(|_| {
                    panic!(
                        "{}: `{O2_CALLS}{marker}` is not `<caller> <callee> <count>`",
                        path.display()
                    )
                })
        })
        .collect();
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
            built.push((outer, o2_ir(path, outer)));
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
fn expect_files(files: &[(PathBuf, String)], bless: bool) {
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
