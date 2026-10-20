//! `sisuc`, the Sisu compiler.

mod ast;
mod check;
mod codegen;
mod diagnostic;
mod front_end;
mod fuzz;
mod lexer;
mod link;
mod mir;
mod parser;
mod report;
mod source;
mod symbol;
mod tir;
mod types;

use std::ffi::OsString;
use std::fs;
use std::io::{self, ErrorKind, IsTerminal, Read, Stderr, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use codegen::OptLevel;
use diagnostic::{Code, Diagnostic, RETIRED, Severity};
use inkwell::context::Context;
use report::{Format, Reporter};
use tir::Program;

pub use fuzz::front_end as fuzz_front_end;

const USAGE: &str = "usage: sisuc [--format text|json] ([-O2|-O3] [--rc-checks] [--linker bfd|gold|lld|mold] <input.sisu> <output> | [-O2|-O3] [--rc-checks] --emit ir <input.sisu> | [--rc-checks] --emit ir-raw <input.sisu> | --emit tokens|ast|tir|mir <input.sisu> | --check <input.sisu>) | sisuc --explain <code>";

/// The reporter `cli` writes stderr through.
type Report = Reporter<Stderr>;

/// What the command line asks for.
enum Mode {
    Emit {
        stage: Stage,
        input: PathBuf,
        level: OptLevel,
        rc_checks: bool,
    },
    Check {
        input: PathBuf,
    },
    /// Print the explanation of a diagnostic code.
    Explain {
        code: String,
    },
    Build {
        input: PathBuf,
        output: PathBuf,
        level: OptLevel,
        rc_checks: bool,
        /// The `-fuse-ld` name `cc` links with; `cc`'s default without one.
        linker: Option<&'static str>,
    },
}

enum Stage {
    Tokens,
    Ast,
    Tir,
    Mir,
    Ir,
    IrRaw,
}

/// The stage `--emit` names.
fn emit_stage(name: &OsString) -> Result<Stage, String> {
    match name.to_str() {
        Some("tokens") => Ok(Stage::Tokens),
        Some("ast") => Ok(Stage::Ast),
        Some("tir") => Ok(Stage::Tir),
        Some("mir") => Ok(Stage::Mir),
        Some("ir") => Ok(Stage::Ir),
        Some("ir-raw") => Ok(Stage::IrRaw),
        _ => Err(USAGE.to_string()),
    }
}

/// The format and the mode the command line asks for.
fn parse_args(args: &[OsString]) -> Result<(Format, Mode), String> {
    // `--format` is accepted only as the first two arguments.
    let (format, args) = match args {
        [flag, format, rest @ ..] if flag == "--format" => match format.to_str() {
            Some("text") => (Some(Format::Text), rest),
            Some("json") => (Some(Format::Json), rest),
            _ => return Err(USAGE.to_string()),
        },
        _ => (None, args),
    };
    // `-O2` or `-O3` is accepted only as the first argument after `--format`.
    let (level, args) = match args {
        [flag, rest @ ..] if flag == "-O2" => (OptLevel::O2, rest),
        [flag, rest @ ..] if flag == "-O3" => (OptLevel::O3, rest),
        _ => (OptLevel::O0, args),
    };
    // `--rc-checks` is accepted only right after an optional `-O2` or `-O3`, and only for a build
    // or IR.
    let (rc_checks, args) = match args {
        [flag, rest @ ..] if flag == "--rc-checks" => (true, rest),
        _ => (false, args),
    };
    // `--linker` is accepted only right after an optional `--rc-checks`, and only for a build;
    // the names are those `cc -fuse-ld` takes.
    let (linker, args) = match args {
        [flag, name, rest @ ..] if flag == "--linker" => {
            let name = ["bfd", "gold", "lld", "mold"]
                .into_iter()
                .find(|&n| *name == n);
            (Some(name.ok_or_else(|| USAGE.to_string())?), rest)
        }
        _ => (None, args),
    };
    if args.iter().any(|a| {
        a == "-O2" || a == "-O3" || a == "--rc-checks" || a == "--linker" || a == "--format"
    }) {
        return Err(USAGE.to_string());
    }
    let mode = match args {
        [flag, stage, input] if flag == "--emit" => Ok(Mode::Emit {
            stage: emit_stage(stage)?,
            input: PathBuf::from(input),
            level,
            rc_checks,
        }),
        [flag, input] if flag == "--check" => Ok(Mode::Check {
            input: PathBuf::from(input),
        }),
        [flag, code] if flag == "--explain" => Ok(Mode::Explain {
            code: code.to_string_lossy().into_owned(),
        }),
        [input, output]
            if [input, output]
                .iter()
                .all(|a| !a.to_string_lossy().starts_with('-')) =>
        {
            Ok(Mode::Build {
                input: PathBuf::from(input),
                output: PathBuf::from(output),
                level,
                rc_checks,
                linker,
            })
        }
        _ => Err(USAGE.to_string()),
    }?;
    if rc_checks
        && !matches!(
            mode,
            Mode::Build { .. }
                | Mode::Emit {
                    stage: Stage::Ir | Stage::IrRaw,
                    ..
                }
        )
    {
        return Err(USAGE.to_string());
    }
    // `--explain` prints to stdout and takes no other flag.
    if format.is_some() && matches!(mode, Mode::Explain { .. }) {
        return Err(USAGE.to_string());
    }
    if linker.is_some() && !matches!(mode, Mode::Build { .. }) {
        return Err(USAGE.to_string());
    }
    // The other modes run no optimizer.
    if level != OptLevel::O0
        && !matches!(
            mode,
            Mode::Build { .. }
                | Mode::Emit {
                    stage: Stage::Ir,
                    ..
                }
        )
    {
        return Err(USAGE.to_string());
    }
    Ok((format.unwrap_or(Format::Text), mode))
}

/// The stack of the thread that runs the compiler. The parser bounds the depth of the syntax tree
/// (`MAX_DEPTH`), and the deepest tree compiles in about 5.5 MiB in a debug build. 64 MiB is
/// four times that and more. The pages are mapped but touched only when used.
pub const STACK_SIZE: usize = 64 << 20;

/// Parses the arguments, does what they ask for, and reports.
pub fn cli() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let (format, mode) = match parse_args(&args) {
        Ok(parsed) => parsed,
        Err(usage) => {
            // The format is not known yet, so this stays plain text.
            eprintln!("{usage}");
            return ExitCode::FAILURE;
        }
    };
    let color = report::use_color(
        io::stderr().is_terminal(),
        std::env::var_os("NO_COLOR").as_deref(),
    );
    let mut reporter = Reporter::new(io::stderr(), format, color);
    let code = run(&mut reporter, mode);
    reporter.finish();
    code
}

/// Does what `mode` asks for.
fn run(reporter: &mut Report, mode: Mode) -> ExitCode {
    match mode {
        Mode::Emit {
            stage,
            input,
            level,
            rc_checks,
        } => emit(reporter, &stage, &input, level, rc_checks),
        Mode::Check { input } => run_check(reporter, &input),
        Mode::Explain { code } => explain(reporter, &code),
        Mode::Build {
            input,
            output,
            level,
            rc_checks,
            linker,
        } => build(reporter, &input, &output, level, rc_checks, linker),
    }
}

/// Prints the explanation of `code`, in any case, retired or not, or that there is no such code.
fn explain(reporter: &mut Report, code: &str) -> ExitCode {
    let retired = || {
        let (_, text) = RETIRED.iter().find(|(c, _)| c.eq_ignore_ascii_case(code))?;
        Some(*text)
    };
    if let Some(text) = Code::parse(code).map(Code::explanation).or_else(retired) {
        print!("{text}");
        ExitCode::SUCCESS
    } else {
        reporter.tool_error(&format!("unknown diagnostic code `{code}`"));
        ExitCode::FAILURE
    }
}

/// Reads the bytes of `input`, at most `MAX_SOURCE` of them, or reports why it cannot.
fn read_bytes(reporter: &mut Report, input: &Path) -> Option<Vec<u8>> {
    let read = fs::File::open(input)
        .and_then(|file| read_at_most(&file, file.metadata()?.len(), source::MAX_SOURCE));
    let path = input.to_string_lossy();
    match read {
        Ok(Some(bytes)) => return Some(bytes),
        Ok(None) => reporter.tool_error(&format!(
            "cannot read {path}: the file is larger than 1 GiB"
        )),
        Err(e) => reporter.tool_error(&format!("cannot read {path}: {e}")),
    }
    None
}

/// The bytes of `reader`, or `None` if there are more than `limit`. `len` is the length the
/// file's metadata gives, 0 for a pipe: a longer one is rejected before reading, and it sizes
/// the buffer. The read stops one byte past `limit`, for a source whose length `len` misses.
fn read_at_most(reader: impl Read, len: u64, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let limit64 =
        u64::try_from(limit).expect("a `usize` should fit in `u64` on every supported target");
    if len > limit64 {
        return Ok(None);
    }
    let capacity = usize::try_from(len)
        .expect("`len` is at most `limit`, a `usize`, so it should fit in `usize`");
    let mut bytes = Vec::with_capacity(capacity);
    reader.take(limit64 + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() <= limit).then_some(bytes))
}

/// Reads `input`, or reports why it cannot. Text that is not UTF-8 is a diagnostic at its first
/// invalid byte, rendered over the valid text before it, so the renderer never sees bad UTF-8.
fn read(reporter: &mut Report, input: &Path) -> Option<String> {
    let path = input.to_string_lossy();
    let bytes = read_bytes(reporter, input)?;
    front_end::decode(bytes)
        .inspect_err(|e| {
            let (shown, d) = &**e;
            reporter.diagnostics(&path, shown, std::slice::from_ref(d));
        })
        .ok()
}

/// Prints one stage's output for `input` to stdout; the IR stages build in count checks with
/// `rc_checks`.
fn emit(
    reporter: &mut Report,
    stage: &Stage,
    input: &Path,
    level: OptLevel,
    rc_checks: bool,
) -> ExitCode {
    let path = input.to_string_lossy();
    let Some(source) = read(reporter, input) else {
        return ExitCode::FAILURE;
    };
    match stage {
        Stage::Tokens => {
            let Some(lexed) = lex(reporter, &path, &source) else {
                return ExitCode::FAILURE;
            };
            if !lexed.diagnostics.is_empty() {
                reporter.diagnostics(&path, &source, &lexed.diagnostics);
                return ExitCode::FAILURE;
            }
            reporter.diagnostics(&path, &source, &lexed.warnings);
            print!("{}", lexer::dump(&source, &lexed.tokens));
            ExitCode::SUCCESS
        }
        Stage::Ast => {
            let Some(lexed) = lex(reporter, &path, &source) else {
                return ExitCode::FAILURE;
            };
            let parsed = parser::parse(lexed);
            reporter.diagnostics(&path, &source, &parsed.diagnostics);
            if parsed
                .diagnostics
                .iter()
                .any(|d| d.severity() == Severity::Error)
            {
                return ExitCode::FAILURE;
            }
            println!("{}", parsed.program);
            ExitCode::SUCCESS
        }
        Stage::Tir => match front_end(reporter, &path, &source) {
            Some(program) => {
                println!("{program}");
                ExitCode::SUCCESS
            }
            None => ExitCode::FAILURE,
        },
        Stage::Mir => match front_end(reporter, &path, &source) {
            Some(program) => {
                print!("{}", mir::print(&mir::build(&program), &program));
                ExitCode::SUCCESS
            }
            None => ExitCode::FAILURE,
        },
        Stage::Ir => emit_ir(reporter, &path, &source, level, rc_checks),
        Stage::IrRaw => emit_ir_raw(reporter, &path, &source, rc_checks),
    }
}

/// Lexes `source`, reporting the error and returning `None` when it cannot be lexed.
fn lex<'a>(reporter: &mut Report, path: &str, source: &'a str) -> Option<lexer::Lexed<'a>> {
    match lexer::lex(source) {
        Ok(lexed) => Some(lexed),
        Err(d) => {
            reporter.diagnostics(path, source, &[d]);
            None
        }
    }
}

/// Prints the module for `source` before and after the pipeline, once it passes the checker.
fn emit_ir(
    reporter: &mut Report,
    path: &str,
    source: &str,
    level: OptLevel,
    rc_checks: bool,
) -> ExitCode {
    let Some(program) = front_end(reporter, path, source) else {
        return ExitCode::FAILURE;
    };
    let mir = mir::build(&program);
    let context = Context::create();
    let pipeline = codegen::pipeline(level);
    let printed = codegen::target_machine(level).and_then(|machine| {
        let module = codegen::compile(&context, &program, &mir, path, source, &machine, rc_checks);
        // For the `o2-calls` check in the tests (`docs/development.md`); `--emit ir` only, so
        // no executable is affected, and not in the usage text.
        if let Ok(names) = std::env::var("SISU_KEEP_FUNCTIONS") {
            codegen::keep_functions(&module, &names);
        }
        // `LLVMString`'s `Display` quotes and escapes the text; `to_string` does not.
        let before = module.print_to_string().to_string();
        codegen::optimize(&module, &machine, level)?;
        Ok(format!(
            "; before {pipeline}\n{before}\n; after {pipeline}\n{}",
            module.print_to_string().to_string()
        ))
    });
    print_or_report(reporter, printed)
}

/// Prints `text`, or reports the error.
fn print_or_report(reporter: &mut Report, text: Result<String, String>) -> ExitCode {
    match text {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            reporter.tool_error(&message);
            ExitCode::FAILURE
        }
    }
}

/// Prints the verified module for `source`, as codegen made it.
fn emit_ir_raw(reporter: &mut Report, path: &str, source: &str, rc_checks: bool) -> ExitCode {
    let Some(program) = front_end(reporter, path, source) else {
        return ExitCode::FAILURE;
    };
    let mir = mir::build(&program);
    let context = Context::create();
    print_or_report(
        reporter,
        codegen::target_machine(OptLevel::O0).and_then(|machine| {
            let module =
                codegen::compile(&context, &program, &mir, path, source, &machine, rc_checks);
            module.verify().map_err(|e| e.to_string())?;
            Ok(module.print_to_string().to_string())
        }),
    )
}

/// Lexes, parses and checks `input`; reports every diagnostic. Fails if any is an error.
fn run_check(reporter: &mut Report, input: &Path) -> ExitCode {
    let Some(bytes) = read_bytes(reporter, input) else {
        return ExitCode::FAILURE;
    };
    if check_bytes(reporter, &input.to_string_lossy(), bytes)
        .program
        .is_some()
    {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// What `sisuc --check` reported for one source.
pub(crate) struct Checked {
    /// The text the diagnostics point into: the source, or for bytes that are not UTF-8,
    /// the replacement text `decode` returns.
    pub(crate) source: String,
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// The program, unless a diagnostic is an error. `--check` passes when it is `Some`.
    pub(crate) program: Option<Program>,
}

/// Decodes `bytes`, runs the front end on the source, and reports every diagnostic as read
/// from `path`. Bytes that are not UTF-8 are the one `InvalidUtf8` diagnostic instead.
///
/// `bytes` are at most `MAX_SOURCE` long, so every span offset fits in a `u32`: `read_bytes`
/// ensures it for the command line, and the fuzz target's `-max_len=16384` for fuzzing.
pub(crate) fn check_bytes<W: Write>(
    reporter: &mut Reporter<W>,
    path: &str,
    bytes: Vec<u8>,
) -> Checked {
    let (source, diagnostics, program) = match front_end::decode(bytes) {
        Ok(source) => {
            let (program, diagnostics) = front_end::front_end(&source);
            (source, diagnostics, program)
        }
        Err(e) => {
            let (shown, d) = *e;
            (shown, vec![d], None)
        }
    };
    reporter.diagnostics(path, &source, &diagnostics);
    Checked {
        source,
        diagnostics,
        program,
    }
}

/// Compiles `input` into the executable `output`.
fn build(
    reporter: &mut Report,
    input: &Path,
    output: &Path,
    level: OptLevel,
    rc_checks: bool,
    linker: Option<&str>,
) -> ExitCode {
    // `cc -o` would replace the source with the executable.
    let overwrites_input = match (fs::canonicalize(input), fs::canonicalize(output)) {
        (Ok(input), Ok(output)) => input == output,
        _ => input == output,
    };
    if overwrites_input {
        reporter.tool_error(&format!(
            "output {} would overwrite the input",
            output.to_string_lossy()
        ));
        return ExitCode::FAILURE;
    }
    // Found here, not by `ld` after a whole compile.
    let runtime = match link::runtime_library().and_then(|runtime| {
        check_output(output)?;
        Ok(runtime)
    }) {
        Ok(runtime) => runtime,
        Err(message) => {
            reporter.tool_error(&message);
            return ExitCode::FAILURE;
        }
    };
    let path = input.to_string_lossy();
    let Some(source) = read(reporter, input) else {
        return ExitCode::FAILURE;
    };
    let Some(program) = front_end(reporter, &path, &source) else {
        return ExitCode::FAILURE;
    };
    let linking = link::Linking {
        runtime: &runtime,
        output,
        linker,
    };
    match compile(&program, &path, &source, &linking, level, rc_checks) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            reporter.tool_error(&message);
            ExitCode::FAILURE
        }
    }
}

/// Checks, without compiling, that `ld` can create `output`: its directory exists and it is not
/// a directory.
fn check_output(output: &Path) -> Result<(), String> {
    let parent = match output.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let shown = parent.display();
    match fs::metadata(parent) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err(format!("output directory {shown} is not a directory")),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(format!("output directory {shown} does not exist"));
        }
        Err(e) => return Err(format!("cannot access output directory {shown}: {e}")),
    }
    if output.is_dir() {
        return Err(format!(
            "output {} is a directory",
            output.to_string_lossy()
        ));
    }
    Ok(())
}

/// Runs the front end on `source` and reports every diagnostic. Returns the program unless a
/// diagnostic is an error.
fn front_end(reporter: &mut Report, path: &str, source: &str) -> Option<Program> {
    let (program, diagnostics) = front_end::front_end(source);
    reporter.diagnostics(path, source, &diagnostics);
    program
}

/// Compiles `program` into the executable `output`, going through a temporary object file;
/// with `rc_checks`, the executable checks its reference counts.
fn compile(
    program: &Program,
    path: &str,
    source: &str,
    linking: &link::Linking,
    level: OptLevel,
    rc_checks: bool,
) -> Result<(), String> {
    let mir = mir::build(program);
    let context = Context::create();
    let machine = codegen::target_machine(level)?;
    let module = codegen::compile(&context, program, &mir, path, source, &machine, rc_checks);
    codegen::optimize(&module, &machine, level)?;
    let code = codegen::object_code(&module, &machine)?;
    // A name of its own, so a user's `<output>.o` is left alone; the nanoseconds pick a
    // fresh name when one is taken.
    let pid = std::process::id();
    let names = (0..10).map(|_| {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        format!("sisuc-{pid}-{nanos}.o")
    });
    let object = create_object(&std::env::temp_dir(), code.as_slice(), names)?;
    // Remove the object file whether or not the link worked; a link error wins.
    let link_result = link::link(&object, linking);
    let removed =
        fs::remove_file(&object).map_err(|e| format!("cannot remove {}: {e}", object.display()));
    link_result.and(removed)
}

/// Writes `bytes` to a new file in `dir`, under the first of `names` that is free, and
/// returns its path.
fn create_object(
    dir: &Path,
    bytes: &[u8],
    names: impl IntoIterator<Item = String>,
) -> Result<PathBuf, String> {
    // `create_new` is `O_CREAT | O_EXCL`: it fails on any existing path, a symlink included,
    // so a name planted in a shared temp dir is never followed or overwritten.
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    // Owner-only from creation: other users of the shared temp dir cannot read the object.
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    for name in names {
        let path = dir.join(name);
        let mut file = match options.open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("cannot create {}: {e}", path.display())),
        };
        return match file.write_all(bytes) {
            Ok(()) => Ok(path),
            Err(e) => {
                let message = format!("cannot write {}: {e}", path.display());
                fs::remove_file(&path)
                    .map_err(|e| format!("{message}; cannot remove it: {e}"))
                    .and(Err(message))
            }
        };
    }
    Err(format!(
        "cannot create a temporary object file in {}",
        dir.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_at_most_stops_one_past_the_limit() {
        struct Failing;
        impl Read for Failing {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("fails"))
            }
        }

        let read = |len: u64, bytes: &[u8]| read_at_most(bytes, len, 4).expect("reads");
        // The metadata length, then the bytes read when it says 0, as for a pipe.
        assert_eq!(read(4, b"abcd").as_deref(), Some(&b"abcd"[..]));
        assert_eq!(read(5, b""), None);
        assert_eq!(read(0, b""), Some(vec![]));
        assert_eq!(read(0, b"abcd").as_deref(), Some(&b"abcd"[..]));
        assert_eq!(read(0, b"abcde"), None);

        // A longer stream is read only one byte past the limit.
        let mut long = io::Cursor::new(vec![0; 64]);
        assert_eq!(read_at_most(&mut long, 0, 4).expect("reads"), None);
        assert_eq!(long.position(), 5);

        let e = read_at_most(Failing, 0, 4).expect_err("the read fails");
        assert_eq!(e.kind(), ErrorKind::Other);
    }

    #[cfg(unix)]
    #[test]
    fn object_file_never_follows_an_existing_path() {
        use std::os::unix::fs::symlink;

        let dir = std::env::temp_dir().join(format!("sisuc-create-object-{}", std::process::id()));
        // It is absent unless an earlier run failed, so a failure here is expected and harmless.
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).expect("creates the directory");
        let target = dir.join("target");
        fs::write(&target, "not sisuc's").expect("writes");
        symlink(&target, dir.join("taken.o")).expect("links");
        symlink(dir.join("absent"), dir.join("dangling.o")).expect("links");

        let names = ["taken.o", "dangling.o", "free.o"].map(String::from);
        let object = create_object(&dir, b"object", names).expect("`free.o` is free");
        assert_eq!(object, dir.join("free.o"));
        assert_eq!(fs::read(&object).expect("reads"), b"object");
        assert_eq!(fs::read_to_string(&target).expect("reads"), "not sisuc's");
        assert!(!dir.join("absent").exists());
        // With every name taken, it gives up without writing.
        assert!(create_object(&dir, b"object", ["taken.o".to_string()]).is_err());
        assert_eq!(fs::read_to_string(&target).expect("reads"), "not sisuc's");
        fs::remove_dir_all(&dir).expect("removes the directory");
    }

    #[cfg(unix)]
    #[test]
    fn object_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("sisuc-object-mode-{}", std::process::id()));
        // It is absent unless an earlier run failed, so a failure here is expected and harmless.
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).expect("creates the directory");
        let object = create_object(&dir, b"object", ["private.o".to_string()]).expect("creates");
        let mode = fs::metadata(&object)
            .expect("reads metadata")
            .permissions()
            .mode();
        fs::remove_dir_all(&dir).expect("removes the directory");
        assert_eq!(mode & 0o077, 0, "mode {mode:o}");
    }
}
