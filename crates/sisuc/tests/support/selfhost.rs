//! The lexer in Sisu, `selfhost/lexer.sisu`: its builds, which a cache shares between the test
//! binaries and the test processes of one run, and the comparison of its output with the output of
//! `sisu build --emit tokens`.

use std::collections::hash_map::DefaultHasher;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::process::Command;
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

/// The builds of `selfhost/lexer.sisu`, made at most once for a test run: the cache under
/// `CARGO_TARGET_TMPDIR` holds them, and the test processes of the run share it.
pub(crate) fn builds() -> &'static Builds {
    static BUILDS: OnceLock<Builds> = OnceLock::new();
    BUILDS.get_or_init(|| {
        let cache = Path::new(env!("CARGO_TARGET_TMPDIR")).join("selfhost");
        build(Path::new(LEXER), &cache)
    })
}

/// The builds of `source` in `cache`. Builds in the cache from the same source, by the same
/// `sisu` and runtime, are reused. A process that finds the cache locked waits: the lock stops two
/// test processes from building into it at once.
pub(crate) fn build(source: &Path, cache: &Path) -> Builds {
    fs::create_dir_all(cache).expect("the cache directory should be creatable");
    let lock = File::create(cache.join("lock")).expect("the lock file should be creatable");
    lock.lock().expect("the cache lock should be held");
    let builds = Builds {
        o0: cache.join("lexer-O0"),
        o2: cache.join("lexer-O2"),
        rc_checks: cache.join("lexer-rc"),
    };
    let stamp_path = cache.join("stamp");
    let stamp = stamp(source);
    if fs::read_to_string(&stamp_path).ok().as_ref() != Some(&stamp) {
        // The stamp goes first and returns last, so that a run that stops half way rebuilds.
        drop(fs::remove_file(&stamp_path));
        let dir = source.parent().expect("the source should have a directory");
        let name = source
            .file_stem()
            .and_then(|name| name.to_str())
            .expect("the source name should be UTF-8");
        for (exe, flag) in [
            (&builds.o0, None),
            (&builds.o2, Some("-O2")),
            (&builds.rc_checks, Some("--rc-checks")),
        ] {
            let flags: Vec<&str> = flag
                .into_iter()
                .chain(VALGRIND_FLAGS.iter().copied())
                .collect();
            let stderr = compile(dir, name, exe, &flags);
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

/// What the builds depend on: a hash of `source`, and the size and modification time of `sisu` and
/// of the two runtime archives beside it. A change to the runtime relinks no `sisu`, so the
/// archives count on their own.
fn stamp(source: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    hasher.write(&fs::read(source).expect("the source should be readable"));
    let mut stamp = format!("{:016x}\n", hasher.finish());
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

/// The standard output of `sisu build --emit tokens <path>`, which must exit 0.
pub(crate) fn tokens_of(path: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .args(["build", "--emit", "tokens"])
        .arg(path)
        .output()
        .expect("sisu starts");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("the tokens should be UTF-8")
}

/// Checks that each build of the lexer, with the file `input` on its standard input, exits 0 with
/// nothing on standard error and prints what `tokens_of(input)` holds, byte for byte. A mismatch
/// names the input and its first differing line.
pub(crate) fn compare(input: &Path) {
    let expected = tokens_of(input);
    let stdin = fs::read(input).expect("the input should be readable");
    let builds = builds();
    for exe in [&builds.o2, &builds.o0, &builds.rc_checks] {
        let out = Invocation::new(stdin.clone(), Vec::new()).run(exe, false);
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
        if let Some(difference) = first_difference(&expected, &String::from_utf8_lossy(&out.stdout))
        {
            panic!(
                "{} on {}: the tokens differ from `sisu build --emit tokens`, at {difference}",
                exe.display(),
                input.display()
            );
        }
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
