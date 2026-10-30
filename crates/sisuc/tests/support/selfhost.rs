//! The lexer in Sisu, `selfhost/lexer.sisu`. This module builds it, and a cache shares the builds
//! between the test binaries and the test processes of one run. It also compares the output of the
//! lexer with the output of `sisu build --emit tokens`, in the corpus check and under Valgrind.

use std::collections::hash_map::DefaultHasher;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use super::{Invocation, VALGRIND_FLAGS, compile};

const LEXER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../selfhost/lexer.sisu");

/// The executables of the lexer: a plain build, an `-O2` build and an `--rc-checks` build. Each
/// uses the allocator that `VALGRIND_FLAGS` names, so that a run under Valgrind can use any of
/// them.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Builds {
    pub(crate) o0: PathBuf,
    pub(crate) o2: PathBuf,
    pub(crate) rc_checks: PathBuf,
}

/// The builds of `selfhost/lexer.sisu`, made at most once for a test run. The cache under
/// `CARGO_TARGET_TMPDIR` holds them, and the test processes of the run share it.
pub(crate) fn builds() -> &'static Builds {
    static BUILDS: OnceLock<Builds> = OnceLock::new();
    BUILDS.get_or_init(|| {
        let cache = Path::new(env!("CARGO_TARGET_TMPDIR")).join("selfhost");
        build(Path::new(LEXER), &cache)
    })
}

/// The builds that `build` makes: the name of each executable, and the flags that `sisu` gets. Each
/// build uses the allocator that `VALGRIND_FLAGS` names.
fn plan() -> [(&'static str, Vec<&'static str>); 3] {
    let with_valgrind_flags = |flag: Option<&'static str>| {
        flag.into_iter()
            .chain(VALGRIND_FLAGS.iter().copied())
            .collect()
    };
    [
        ("lexer-O0", with_valgrind_flags(None)),
        ("lexer-O2", with_valgrind_flags(Some("-O2"))),
        ("lexer-rc", with_valgrind_flags(Some("--rc-checks"))),
    ]
}

/// The builds of `source` in `cache`. Builds in the cache from the same source, by the same
/// `sisu` and runtime, are reused. A process that finds the cache locked waits: the lock stops two
/// test processes from building into it at once.
pub(crate) fn build(source: &Path, cache: &Path) -> Builds {
    fs::create_dir_all(cache).expect("the cache directory should be creatable");
    let lock = File::create(cache.join("lock")).expect("the lock file should be creatable");
    lock.lock().expect("the cache lock should be held");
    let plan = plan();
    let builds = Builds {
        o0: cache.join(plan[0].0),
        o2: cache.join(plan[1].0),
        rc_checks: cache.join(plan[2].0),
    };
    let stamp_path = cache.join("stamp");
    let stamp = stamp(source, &plan);
    if fs::read_to_string(&stamp_path).ok().as_ref() != Some(&stamp) {
        // The stamp goes first and returns last, so that a run that stops half way rebuilds.
        drop(fs::remove_file(&stamp_path));
        let dir = source.parent().expect("the source should have a directory");
        let name = source
            .file_stem()
            .and_then(|name| name.to_str())
            .expect("the source name should be UTF-8");
        for (file, flags) in &plan {
            let stderr = compile(dir, name, &cache.join(file), flags);
            assert!(
                stderr.is_empty(),
                "{} gives warnings with {flags:?}:\n{}",
                source.display(),
                String::from_utf8_lossy(&stderr)
            );
        }
        fs::write(&stamp_path, stamp).expect("the stamp should be writable");
    }
    builds
}

/// What the builds depend on. It holds a hash of `source` and the name and flags of each build in
/// `plan`. It also holds the size and modification time of `sisu` and of the two runtime archives
/// beside it. A change to the runtime relinks no `sisu`, so the archives count on their own.
fn stamp(source: &Path, plan: &[(&str, Vec<&str>)]) -> String {
    let mut hasher = DefaultHasher::new();
    hasher.write(&fs::read(source).expect("the source should be readable"));
    let mut stamp = format!("{:016x}\n", hasher.finish());
    for (file, flags) in plan {
        writeln!(stamp, "{file} {flags:?}").expect("writing to a `String` should never fail");
    }
    let sisu = Path::new(env!("CARGO_BIN_EXE_sisu"));
    for file in [
        sisu.to_path_buf(),
        sisu.with_file_name("libsisu_runtime_mimalloc.a"),
        sisu.with_file_name("libsisu_runtime_system.a"),
    ] {
        let metadata = fs::metadata(&file).expect("the executable should have metadata");
        let modified = metadata
            .modified()
            .expect("the executable should have a modification time")
            .duration_since(UNIX_EPOCH)
            .expect("a modification time should be after 1970")
            .as_nanos();
        writeln!(stamp, "{} {} {modified}", file.display(), metadata.len())
            .expect("writing to a `String` should never fail");
    }
    stamp
}

/// What the corpus check does with one file.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// `sisu build --emit tokens` exited 0, warnings allowed. These are the tokens it printed.
    Compared(String),
    /// It exited 1 with a diagnostic code: the file does not lex, so no lexer need print tokens.
    Excluded,
}

/// Runs `sisu build --emit tokens` on `path` and classifies the file. Any other result fails the
/// check, with the exit status of `sisu`: a panic, a signal, or an exit 1 with no diagnostic code
/// is a fault in `sisu`, not a file the lexer may skip.
pub(crate) fn selfhost_outcome(path: &Path) -> Outcome {
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .args(["build", "--emit", "tokens"])
        .arg(path)
        .output()
        .expect("sisu starts");
    let stderr = String::from_utf8_lossy(&out.stderr);
    match out.status.code() {
        Some(0) => {
            Outcome::Compared(String::from_utf8(out.stdout).expect("the tokens should be UTF-8"))
        }
        Some(1) if stderr.contains("error[E") => Outcome::Excluded,
        _ => panic!(
            "{}: `sisu build --emit tokens` ended with {}:\n{stderr}",
            path.display(),
            out.status
        ),
    }
}

/// The standard output of `sisu build --emit tokens <path>`, which must exit 0.
pub(crate) fn tokens_of(path: &Path) -> String {
    match selfhost_outcome(path) {
        Outcome::Compared(tokens) => tokens,
        Outcome::Excluded => panic!("{}: the file does not lex", path.display()),
    }
}

/// Runs `exe`, a build of the lexer, with the file `input` on its standard input.
fn run_lexer(exe: &Path, input: &Path, valgrind: bool) -> Output {
    let stdin = fs::read(input).expect("the input should be readable");
    Invocation::new(stdin, Vec::new()).run(exe, valgrind)
}

/// Runs each build of the lexer with the file `input` on its standard input. Each must exit 0 with
/// nothing on standard error. Each must print `expected`, the tokens of `input` from
/// `sisu build --emit tokens`, byte for byte. A mismatch names the input and its first differing
/// line.
pub(crate) fn compare(input: &Path, expected: &str) {
    let builds = builds();
    for exe in [&builds.o2, &builds.o0, &builds.rc_checks] {
        let out = run_lexer(exe, input, false);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{} on {}: {stderr}",
            exe.display(),
            input.display()
        );
        assert_eq!(
            stderr,
            "",
            "{} on {}: writes to stderr",
            exe.display(),
            input.display()
        );
        assert_same_tokens(exe, input, expected, &out);
    }
}

/// Runs `exe`, a build of the lexer, under Valgrind with the file `input` on its standard input.
/// It must exit 0, so it leaks nothing and makes no memory error, and print the tokens that
/// `sisu build --emit tokens` prints. A failure shows the standard error of Valgrind.
pub(crate) fn compare_under_valgrind(exe: &Path, input: &Path) {
    let out = run_lexer(exe, input, true);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{} on {}: {stderr}",
        exe.display(),
        input.display()
    );
    assert_same_tokens(exe, input, &tokens_of(input), &out);
}

/// Panics, naming the first differing line, unless the standard output of `out` is `expected`.
fn assert_same_tokens(exe: &Path, input: &Path, expected: &str, out: &Output) {
    if let Some(difference) = first_difference(expected, &String::from_utf8_lossy(&out.stdout)) {
        panic!(
            "{} on {}: the tokens differ from `sisu build --emit tokens`, at {difference}\n{}",
            exe.display(),
            input.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// The first line where `actual` differs from `expected`, with both lines, or `None` when the
/// texts are equal.
fn first_difference(expected: &str, actual: &str) -> Option<String> {
    let expected: Vec<&str> = expected.split('\n').collect();
    let actual: Vec<&str> = actual.split('\n').collect();
    let at = (0..expected.len().max(actual.len())).find(|&i| expected.get(i) != actual.get(i))?;
    Some(format!(
        "line {}: expected {:?}, found {:?}",
        at + 1,
        expected.get(at),
        actual.get(at)
    ))
}
