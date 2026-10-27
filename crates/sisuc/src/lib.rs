//! `sisuc`, the Sisu compiler.

mod args;
mod ast;
mod check;
mod codegen;
mod conformance;
mod diagnostic;
mod front_end;
mod fuzz;
#[cfg(any(test, feature = "fuzz"))]
mod fuzz_program;
#[cfg(any(test, feature = "fuzz"))]
mod generate;
mod graph;
mod lexer;
mod link;
mod mir;
mod mono;
mod parser;
mod report;
mod run;
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
use inkwell::targets::TargetMachine;
use report::{Format, Reporter};
use tir::Program;

pub use fuzz::front_end as fuzz_front_end;
#[cfg(feature = "fuzz")]
pub use fuzz_program::program as fuzz_program;

/// The reporter `cli` writes stderr through.
type Report = Reporter<Stderr>;

/// What the command line asks for.
enum Mode {
    /// Print the usage.
    Help,
    /// Print the version, and with `verbose` the commit, the host and the LLVM version.
    Version {
        verbose: bool,
    },
    Emit {
        stage: Stage,
        input: Source,
        level: OptLevel,
        rc_checks: bool,
    },
    Check {
        source: Source,
    },
    /// Print the explanation of a diagnostic code.
    Explain {
        code: String,
    },
    Build {
        input: Source,
        output: PathBuf,
        backend: Backend,
    },
    /// Compile the program and run it with `args`.
    Run {
        source: Source,
        args: Vec<OsString>,
        backend: Backend,
    },
}

/// Where the program's text comes from.
enum Source {
    File(PathBuf),
    /// An inline program, from `-e`.
    Inline(OsString),
}

/// How an executable is built: the optimization level, the count checks, the linker, and the
/// allocator.
struct Backend {
    level: OptLevel,
    rc_checks: bool,
    /// The `-fuse-ld` name `cc` links with; `cc`'s default without one.
    linker: Option<String>,
    /// The allocator of the runtime the executable links; `None` is mimalloc.
    allocator: Option<Allocator>,
}

/// The allocator a compiled program allocates with: the runtime archive it links.
#[derive(Clone, Copy, Default, clap::ValueEnum)]
enum Allocator {
    #[default]
    Mimalloc,
    System,
}

/// The stage `--emit` prints.
#[derive(Clone, clap::ValueEnum)]
enum Stage {
    Tokens,
    Ast,
    Tir,
    Mir,
    Ir,
    IrRaw,
}

/// The stack of the thread that runs the compiler. The parser bounds the depth of the syntax tree
/// (`MAX_DEPTH`), and the deepest tree compiles in about 5.5 MiB in a debug build. 64 MiB is
/// four times that and more. The pages are mapped but touched only when used.
pub const STACK_SIZE: usize = 64 << 20;

/// Parses the arguments, does what they ask for, and reports.
pub fn cli() -> ExitCode {
    let (format, mode) = match args::parse(std::env::args_os().collect()) {
        Ok(parsed) => parsed,
        Err(e) => {
            // Not `e.exit()`, which would end the process from the compiler's thread. A usage
            // error that cannot reach stderr has nowhere to be reported, so it only sets the exit
            // code.
            if let Err(write) = e.print()
                && !e.use_stderr()
                && write.kind() != ErrorKind::BrokenPipe
            {
                // A help or version that did not reach stdout is not a success, unless the
                // reader left (`sisu --help | head -1`), which ends quietly as in coreutils.
                report::tool_error_after_summary(
                    &Format::Text,
                    &format!("cannot write to stdout: {write}"),
                );
                return ExitCode::FAILURE;
            }
            // 0 for help, 2 for a usage error.
            return ExitCode::from(u8::try_from(e.exit_code()).unwrap_or(2));
        }
    };
    let color = report::use_color(
        io::stderr().is_terminal(),
        std::env::var_os("NO_COLOR").as_deref(),
    );
    let reporter = Reporter::new(io::stderr(), format.clone(), color);
    execute(reporter, &format, mode)
}

/// Does what `mode` asks for, and finishes `reporter`, which writes in `format`.
fn execute(mut reporter: Report, format: &Format, mode: Mode) -> ExitCode {
    let code = match mode {
        Mode::Help => print_stdout(&mut reporter, &args::help().to_string()),
        Mode::Version { verbose } => print_stdout(&mut reporter, &version_text(verbose)),
        Mode::Emit {
            stage,
            input,
            level,
            rc_checks,
        } => emit(&mut reporter, &stage, &input, level, rc_checks),
        Mode::Check { source } => run_check(&mut reporter, &source),
        Mode::Explain { code } => explain(&mut reporter, &code),
        Mode::Build {
            input,
            output,
            backend,
        } => build(&mut reporter, &input, &output, &backend),
        // `run` finishes the reporter itself, before the program starts.
        Mode::Run {
            source,
            args,
            backend,
        } => return run::run(reporter, format, &source, &args, &backend),
    };
    reporter.finish();
    code
}

/// Writes `text` to stdout, or reports why it cannot.
fn print_stdout(reporter: &mut Report, text: &str) -> ExitCode {
    let mut stdout = io::stdout().lock();
    match write!(stdout, "{text}").and_then(|()| stdout.flush()) {
        // A reader that left (`sisu explain E0001 | head -1`) ends the output quietly.
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.kind() == ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            reporter.tool_error(&format!("cannot write to stdout: {e}"));
            ExitCode::FAILURE
        }
    }
}

/// The text of `--version`, with the commit, host and LLVM lines if `verbose`.
fn version_text(verbose: bool) -> String {
    let name = args::version();
    if !verbose {
        return format!("{name}\n");
    }
    let (major, minor, patch) = inkwell::support::get_llvm_version();
    let host = TargetMachine::get_default_triple();
    format!(
        "{name}\ncommit: {}\nhost: {}\nllvm: {major}.{minor}.{patch}\n",
        env!("SISU_COMMIT"),
        host.as_str().to_string_lossy()
    )
}

/// Prints the explanation of `code`, in any case, retired or not, or that there is no such code.
fn explain(reporter: &mut Report, code: &str) -> ExitCode {
    let retired = || {
        let (_, text) = RETIRED.iter().find(|(c, _)| c.eq_ignore_ascii_case(code))?;
        Some(*text)
    };
    if let Some(text) = Code::parse(code).map(Code::explanation).or_else(retired) {
        print_stdout(reporter, text)
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

/// The name diagnostics and panics give `input`, and its text, or why it cannot be read. Text
/// that is not UTF-8 is a diagnostic at its first invalid byte, rendered over the valid text
/// before it, so the renderer never sees bad UTF-8.
fn read_source(reporter: &mut Report, input: &Source) -> Option<(String, String)> {
    let (name, bytes) = match input {
        Source::File(path) => (
            path.to_string_lossy().into_owned(),
            read_bytes(reporter, path)?,
        ),
        Source::Inline(code) => ("<inline>".to_owned(), code.as_encoded_bytes().to_vec()),
    };
    front_end::decode(bytes)
        .inspect_err(|e| {
            let (shown, d) = &**e;
            reporter.diagnostics(&name, shown, std::slice::from_ref(d));
        })
        .ok()
        .map(|text| (name, text))
}

/// Prints one stage's output for `input` to stdout; the IR stages build in count checks with
/// `rc_checks`.
fn emit(
    reporter: &mut Report,
    stage: &Stage,
    input: &Source,
    level: OptLevel,
    rc_checks: bool,
) -> ExitCode {
    let Some((path, source)) = read_source(reporter, input) else {
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
            print_stdout(reporter, &lexer::dump(&source, &lexed.tokens))
        }
        Stage::Ast => {
            let Some(lexed) = lex(reporter, &path, &source) else {
                return ExitCode::FAILURE;
            };
            let parsed = parser::parse(lexed, false);
            reporter.diagnostics(&path, &source, &parsed.diagnostics);
            if parsed
                .diagnostics
                .iter()
                .any(|d| d.severity() == Severity::Error)
            {
                return ExitCode::FAILURE;
            }
            print_stdout(reporter, &format!("{}\n", parsed.program))
        }
        Stage::Tir => match front_end(reporter, &path, &source) {
            Some(program) => print_stdout(reporter, &format!("{program}\n")),
            None => ExitCode::FAILURE,
        },
        Stage::Mir => match front_end(reporter, &path, &source) {
            Some(program) => print_stdout(reporter, &mir::print(&mir::build(&program), &program)),
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
        Ok(text) => print_stdout(reporter, &text),
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
fn run_check(reporter: &mut Report, input: &Source) -> ExitCode {
    let Some((name, text)) = read_source(reporter, input) else {
        return ExitCode::FAILURE;
    };
    let (program, diagnostics) = front_end::checked(&text);
    reporter.diagnostics(&name, &text, &diagnostics);
    if program.is_some() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// What `sisu check` reported for one source.
pub(crate) struct Checked {
    /// The text the diagnostics point into: the source, or for bytes that are not UTF-8,
    /// the replacement text `decode` returns.
    pub(crate) source: String,
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// The program, unless a diagnostic is an error. `check` passes when it is `Some`.
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
fn build(reporter: &mut Report, input: &Source, output: &Path, backend: &Backend) -> ExitCode {
    // `cc -o` would replace the source with the executable.
    let overwrites_input = match input {
        Source::File(input) => match (fs::canonicalize(input), fs::canonicalize(output)) {
            (Ok(input), Ok(output)) => input == output,
            _ => input == output,
        },
        Source::Inline(_) => false,
    };
    if overwrites_input {
        reporter.tool_error(&format!(
            "output {} would overwrite the input",
            output.to_string_lossy()
        ));
        return ExitCode::FAILURE;
    }
    // Found here, not by `ld` after a whole compile.
    let allocator = backend.allocator.unwrap_or_default();
    let runtime = match link::runtime_library(allocator).and_then(|runtime| {
        check_output(output)?;
        Ok(runtime)
    }) {
        Ok(runtime) => runtime,
        Err(message) => {
            reporter.tool_error(&message);
            return ExitCode::FAILURE;
        }
    };
    let Some((path, source)) = read_source(reporter, input) else {
        return ExitCode::FAILURE;
    };
    let Some(program) = front_end(reporter, &path, &source) else {
        return ExitCode::FAILURE;
    };
    let compiled = compile(
        &program,
        &path,
        &source,
        &runtime,
        output,
        &std::env::temp_dir(),
        backend,
    );
    match compiled {
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

/// Compiles `program` into the executable `output`, linked with `runtime`, going through a
/// temporary object file in `dir`.
fn compile(
    program: &Program,
    path: &str,
    source: &str,
    runtime: &Path,
    output: &Path,
    dir: &Path,
    backend: &Backend,
) -> Result<(), String> {
    let Backend {
        level,
        rc_checks,
        ref linker,
        allocator: _,
    } = *backend;
    let mir = mir::build(program);
    let context = Context::create();
    let machine = codegen::target_machine(level)?;
    let module = codegen::compile(&context, program, &mir, path, source, &machine, rc_checks);
    codegen::optimize(&module, &machine, level)?;
    let code = codegen::object_code(&module, &machine)?;
    // A name of its own, so a user's `<output>.o` is left alone.
    let object = create_object(dir, code.as_slice(), fresh_names(".o"))?;
    // Remove the object file whether or not the link worked; a link error wins.
    let linking = link::Linking {
        runtime,
        output,
        linker: linker.as_deref(),
    };
    let link_result = link::link(&object, &linking);
    let removed =
        fs::remove_file(&object).map_err(|e| format!("cannot remove {}: {e}", object.display()));
    link_result.and(removed)
}

/// Ten names for a new file or directory in the temp dir: `sisu-<pid>-<nanoseconds><suffix>`.
/// The nanoseconds pick a fresh name when one is taken.
fn fresh_names(suffix: &'static str) -> impl Iterator<Item = String> {
    let pid = std::process::id();
    (0..10).map(move |_| {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        format!("sisu-{pid}-{nanos}{suffix}")
    })
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

        let dir = std::env::temp_dir().join(format!("sisu-create-object-{}", std::process::id()));
        // It is absent unless an earlier run failed, so a failure here is expected and harmless.
        drop(fs::remove_dir_all(&dir));
        fs::create_dir(&dir).expect("creates the directory");
        let target = dir.join("target");
        fs::write(&target, "not sisu's").expect("writes");
        symlink(&target, dir.join("taken.o")).expect("links");
        symlink(dir.join("absent"), dir.join("dangling.o")).expect("links");

        let names = ["taken.o", "dangling.o", "free.o"].map(String::from);
        let object = create_object(&dir, b"object", names).expect("`free.o` is free");
        assert_eq!(object, dir.join("free.o"));
        assert_eq!(fs::read(&object).expect("reads"), b"object");
        assert_eq!(fs::read_to_string(&target).expect("reads"), "not sisu's");
        assert!(!dir.join("absent").exists());
        // With every name taken, it gives up without writing.
        assert!(create_object(&dir, b"object", ["taken.o".to_string()]).is_err());
        assert_eq!(fs::read_to_string(&target).expect("reads"), "not sisu's");
        fs::remove_dir_all(&dir).expect("removes the directory");
    }

    #[cfg(unix)]
    #[test]
    fn object_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("sisu-object-mode-{}", std::process::id()));
        // It is absent unless an earlier run failed, so a failure here is expected and harmless.
        drop(fs::remove_dir_all(&dir));
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
