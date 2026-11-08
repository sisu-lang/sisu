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
mod lang;
mod lexer;
mod link;
mod mir;
mod mono;
mod parser;
mod payload_limit;
mod report;
mod run;
mod source;
mod symbol;
mod timings;
mod tir;
mod types;

use std::ffi::{CString, OsString};
use std::fs;
use std::io::{self, ErrorKind, IsTerminal, Read, Stderr, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use codegen::OptLevel;
use codegen::units::Unit;
use diagnostic::{Code, Diagnostic, RETIRED, Severity};
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::targets::TargetMachine;
use report::{Format, Reporter};
use timings::Timings;
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
    Version { verbose: bool },
    Emit {
        stage: Stage,
        input: Source,
        level: OptLevel,
        rc_checks: bool,
        /// Print the time of each stage.
        timings: bool,
        /// The number of codegen units for `ir` and `ir-raw`; one without it.
        units: Option<usize>,
    },
    Check {
        source: Source,
        /// Print the time of each stage.
        timings: bool,
    },
    /// Print the explanation of a diagnostic code.
    Explain { code: String },
    Build {
        input: Source,
        output: PathBuf,
        backend: Backend,
        /// Print the time of each stage.
        timings: bool,
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
    /// The most codegen units to split the program into; `None` lets the size of the program
    /// decide.
    units: Option<usize>,
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

/// Runs `f` on a thread with the stack of the compiler, as `main` does. A debug build of the
/// parser uses about 30 KiB of stack for each level of nesting, so a test that parses a program
/// some 60 levels deep overflows the 2 MiB stack of a test thread, first on ARM64.
#[cfg(test)]
pub(crate) fn on_compiler_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(STACK_SIZE)
            .spawn_scoped(scope, f)
            .expect("the thread should start")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

/// Whether `sisu` runs as the command-line tool, which exits when its command ends. `cli` sets
/// it, except for `sisu run`, which waits for the program and so frees the compiler's memory
/// first. A fuzzer or a test that compiles again and again in one process leaves it clear.
static EXITS_AFTER: AtomicBool = AtomicBool::new(false);

/// Drops `value`, except in the command-line tool. There the process exits soon after. The OS then
/// frees all of its memory in one step, which is faster than a drop that frees each node.
fn release<T>(value: T) {
    if EXITS_AFTER.load(Ordering::Relaxed) {
        std::mem::forget(value);
    } else {
        drop(value);
    }
}

/// Parses the arguments, does what they ask for, and reports.
pub fn cli() -> ExitCode {
    let (format, mode) = match args::parse(&std::env::args_os().collect::<Vec<_>>()) {
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
    EXITS_AFTER.store(!matches!(mode, Mode::Run { .. }), Ordering::Relaxed);
    let color = report::use_color(
        io::stderr().is_terminal(),
        std::env::var_os("NO_COLOR").as_deref(),
    );
    let reporter = Reporter::new(io::stderr(), format.clone(), color);
    execute(reporter, &format, mode)
}

/// Does what `mode` asks for, and finishes `reporter`, which writes in `format`.
fn execute(mut reporter: Report, format: &Format, mode: Mode) -> ExitCode {
    let mut timings = match mode {
        Mode::Emit { timings: true, .. }
        | Mode::Check { timings: true, .. }
        | Mode::Build { timings: true, .. } => Timings::start(),
        _ => Timings::off(),
    };
    let code = match mode {
        Mode::Help => print_stdout(&mut reporter, &args::help().to_string()),
        Mode::Version { verbose } => print_stdout(&mut reporter, &version_text(verbose)),
        Mode::Emit {
            stage,
            input,
            level,
            rc_checks,
            units,
            ..
        } => emit(
            &mut reporter,
            &stage,
            &input,
            level,
            rc_checks,
            units,
            &mut timings,
        ),
        Mode::Check { source, .. } => run_check(&mut reporter, &source, &mut timings),
        Mode::Explain { code } => explain(&mut reporter, &code),
        Mode::Build {
            input,
            output,
            backend,
            ..
        } => build(&mut reporter, &input, &output, &backend, &mut timings),
        // `run` finishes the reporter itself, before the program starts.
        Mode::Run {
            source,
            args,
            backend,
        } => return run::run(reporter, format, &source, &args, &backend),
    };
    reporter.timings(&timings.report());
    // A report that did not reach stderr is not a success.
    if reporter.finish() {
        code
    } else {
        ExitCode::FAILURE
    }
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
/// `rc_checks`, and in `units` codegen units, one without it.
fn emit(
    reporter: &mut Report,
    stage: &Stage,
    input: &Source,
    level: OptLevel,
    rc_checks: bool,
    units: Option<usize>,
    timings: &mut Timings,
) -> ExitCode {
    let Some((path, source)) = read_source(reporter, input) else {
        return ExitCode::FAILURE;
    };
    match stage {
        Stage::Tokens => {
            let Some(lexed) = lex(reporter, &path, &source, timings) else {
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
            let Some(lexed) = lex(reporter, &path, &source, timings) else {
                return ExitCode::FAILURE;
            };
            let parsed = timings.time(timings::Stage::Parse, || parser::parse(lexed, false));
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
        Stage::Tir => match front_end(reporter, &path, &source, timings) {
            Some(program) => {
                let path = if program.fast_path {
                    "fast path"
                } else {
                    "instantiated"
                };
                print_stdout(reporter, &format!("// mono: {path}\n{program}\n"))
            }
            None => ExitCode::FAILURE,
        },
        Stage::Mir => match front_end(reporter, &path, &source, timings) {
            Some(program) => {
                let mir = timings.time(timings::Stage::Mir, || mir::build(&program));
                print_stdout(reporter, &mir::print(&mir, &program))
            }
            None => ExitCode::FAILURE,
        },
        Stage::Ir => emit_ir(reporter, &path, &source, level, rc_checks, units, timings),
        Stage::IrRaw => emit_ir_raw(reporter, &path, &source, rc_checks, units, timings),
    }
}

/// Lexes `source`, reporting the error and returning `None` when it cannot be lexed.
fn lex<'a>(
    reporter: &mut Report,
    path: &str,
    source: &'a str,
    timings: &mut Timings,
) -> Option<lexer::Lexed<'a>> {
    match timings.time(timings::Stage::Lex, || lexer::lex(source)) {
        Ok(lexed) => {
            timings.count(|| timings::counts(source, &lexed));
            Some(lexed)
        }
        Err(d) => {
            reporter.diagnostics(path, source, &[d]);
            None
        }
    }
}

/// What `each_unit` needs to build the units of one program.
struct UnitInput<'src> {
    program: &'src Program,
    mir: &'src mir::Program,
    /// The name of the source in panic messages.
    path: &'src str,
    source: &'src str,
    level: OptLevel,
    rc_checks: bool,
    /// Whether the values of the IR keep their names, such as `%call`. Only the IR text shows them.
    #[expect(dead_code, reason = "the next commit reads it")]
    keep_names: bool,
}

/// Builds each of `units` and runs `finish` on its index, its module and its target machine, and
/// returns the results in unit order. With one unit, both run on the calling thread, and the
/// module is `codegen::compile`'s. With several, each unit has its own context and target
/// machine, and runs on a thread of `units::schedule`. `finish` runs on that thread, before the
/// module goes. The time of each unit is `timings`' stage `Codegen`, with the stages that
/// `finish` records, and the slowest unit counts (`Timings::merge_units`).
///
/// A unit that fails has an `Err`. The caller reports the lowest index.
fn each_unit<T: Send>(
    input: &UnitInput<'_>,
    units: &[Unit],
    timings: &mut Timings,
    finish: impl Fn(usize, &Module<'_>, &TargetMachine, &mut Timings) -> Result<T, String> + Sync,
) -> Vec<Result<T, String>> {
    // Once, on this thread; the units only create target machines.
    if let Err(message) = codegen::initialize_native_target() {
        return units.iter().map(|_| Err(message.clone())).collect();
    }
    let analyses = (units.len() > 1).then(|| codegen::Analyses::new(input.program, input.source));
    let parent = &*timings;
    let built = codegen::units::schedule(
        units.len(),
        codegen::units::threads(units.len()),
        codegen::units::Order::from_env(),
        |at| {
            let mut unit_timings = parent.for_unit();
            let result = codegen::create_target_machine(input.level).and_then(|machine| {
                let context = Context::create();
                let module = unit_timings.time(timings::Stage::Codegen, || match &analyses {
                    None => codegen::compile(
                        &context,
                        input.program,
                        input.mir,
                        input.path,
                        input.source,
                        &machine,
                        input.rc_checks,
                    ),
                    Some(analyses) => codegen::compile_unit(
                        &context,
                        input.program,
                        input.mir,
                        analyses,
                        &units[at],
                        input.path,
                        input.source,
                        &machine,
                        input.rc_checks,
                    ),
                });
                let done = finish(at, &module, &machine, &mut unit_timings);
                // The module goes before the context that owns it.
                release(module);
                release(context);
                done
            });
            (result, unit_timings)
        },
    );
    let (results, recorded): (Vec<_>, Vec<_>) = built.into_iter().unzip();
    timings.merge_units(recorded);
    results
}

/// Prints the module for `source` before and after the pipeline, once it passes the checker. With
/// `units`, each codegen unit prints in turn (`join_units`).
fn emit_ir(
    reporter: &mut Report,
    path: &str,
    source: &str,
    level: OptLevel,
    rc_checks: bool,
    units: Option<usize>,
    timings: &mut Timings,
) -> ExitCode {
    let Some(program) = front_end(reporter, path, source, timings) else {
        return ExitCode::FAILURE;
    };
    let mir = timings.time(timings::Stage::Mir, || mir::build(&program));
    let pipeline = codegen::pipeline(level);
    let printed = each_ir_unit(
        &UnitInput {
            program: &program,
            mir: &mir,
            path,
            source,
            level,
            rc_checks,
            keep_names: true,
        },
        units,
        timings,
        |_, module, machine, timings| {
            // For the `o2-calls` check in the tests (`docs/development.md`); `--emit ir` only, so
            // no executable is affected, and not in the usage text.
            if let Ok(names) = std::env::var("SISU_KEEP_FUNCTIONS") {
                codegen::keep_functions(module, &names);
            }
            // `LLVMString`'s `Display` quotes and escapes the text; `to_string` does not.
            let before = module.print_to_string().to_string();
            timings.time(timings::Stage::Optimize, || {
                codegen::optimize(module, machine, level)
            })?;
            Ok(format!(
                "; before {pipeline}\n{before}\n; after {pipeline}\n{}",
                module.print_to_string().to_string()
            ))
        },
    );
    print_or_report(reporter, printed)
}

/// Builds the codegen units of `input`, `units` of them, or one without it, and gives the text
/// that `print` makes of each unit's module. The text of one unit is the text of the unit
/// alone. With several, each text follows the line `; codegen unit <k> of <n>`, from 0, and the
/// texts are joined with one empty line. The error of the lowest unit that failed is the result.
fn each_ir_unit(
    input: &UnitInput<'_>,
    units: Option<usize>,
    timings: &mut Timings,
    print: impl Fn(usize, &Module<'_>, &TargetMachine, &mut Timings) -> Result<String, String> + Sync,
) -> Result<String, String> {
    let parts = codegen::units::split(
        input.mir,
        input.program,
        Some(units.unwrap_or(1)),
        input.level,
    );
    each_unit(input, &parts, timings, print)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map(join_units)
}

/// The text of one unit alone, or the text of each of several units after its header, joined
/// with one empty line.
fn join_units(texts: Vec<String>) -> String {
    let count = texts.len();
    match <[String; 1]>::try_from(texts) {
        Ok([text]) => text,
        Err(texts) => (texts.iter().enumerate())
            .map(|(at, text)| format!("; codegen unit {at} of {count}\n{text}"))
            .collect::<Vec<_>>()
            .join("\n"),
    }
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
fn emit_ir_raw(
    reporter: &mut Report,
    path: &str,
    source: &str,
    rc_checks: bool,
    units: Option<usize>,
    timings: &mut Timings,
) -> ExitCode {
    let Some(program) = front_end(reporter, path, source, timings) else {
        return ExitCode::FAILURE;
    };
    let mir = timings.time(timings::Stage::Mir, || mir::build(&program));
    let printed = each_ir_unit(
        &UnitInput {
            program: &program,
            mir: &mir,
            path,
            source,
            level: OptLevel::O0,
            rc_checks,
            keep_names: true,
        },
        units,
        timings,
        |_, module, _, _| {
            module.verify().map_err(|e| e.to_string())?;
            Ok(module.print_to_string().to_string())
        },
    );
    print_or_report(reporter, printed)
}

/// Lexes, parses and checks `input`; reports every diagnostic. Fails if any is an error.
fn run_check(reporter: &mut Report, input: &Source, timings: &mut Timings) -> ExitCode {
    let Some((name, text)) = read_source(reporter, input) else {
        return ExitCode::FAILURE;
    };
    let (program, diagnostics) =
        front_end::checked_timed(front_end::PRELUDE_SOURCE, &text, timings);
    reporter.diagnostics(&name, &text, &diagnostics);
    let checked = program.is_some();
    release(program);
    if checked {
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
fn build(
    reporter: &mut Report,
    input: &Source,
    output: &Path,
    backend: &Backend,
    timings: &mut Timings,
) -> ExitCode {
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
        check_temp_dir()?;
        Ok(runtime)
    }) {
        Ok(runtime) => runtime,
        Err(message) => {
            reporter.tool_error(&message);
            return ExitCode::FAILURE;
        }
    };
    if let Err(message) = link::probe_linker(backend.linker.as_deref()) {
        reporter.tool_error(&message);
        return ExitCode::FAILURE;
    }
    let Some((path, source)) = read_source(reporter, input) else {
        return ExitCode::FAILURE;
    };
    let Some(program) = front_end(reporter, &path, &source, timings) else {
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
        timings,
    );
    release(program);
    match compiled {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            reporter.tool_error(&message);
            ExitCode::FAILURE
        }
    }
}

/// Checks, without compiling, that `ld` can create `output`: its directory exists, and it is not
/// a directory. What else it checks depends on what `output` is, because the linkers differ:
/// - Nothing there: the directory needs write and search permission.
/// - A link: no check. GNU `ld` (bfd and gold) opens the target and does not touch the directory
///   of the link. A dangling link can point into another directory. mold and lld replace the
///   link through its directory, so the linker decides.
/// - Another entry: `ld` writes it, or replaces it through its directory. The check fails only
///   when neither can be done.
///
/// An output in a sticky directory, such as `/tmp`, that another user owns passes this check and
/// can still fail late.
fn check_output(output: &Path) -> Result<(), String> {
    let parent = match output.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    check_directory(parent, "output directory")?;
    if output.is_dir() {
        return Err(format!(
            "output {} is a directory",
            output.to_string_lossy()
        ));
    }
    let directory = || check_access(parent, libc::W_OK | libc::X_OK, "output directory");
    match fs::symlink_metadata(output) {
        Err(_) => directory(),
        Ok(meta) if meta.is_symlink() => Ok(()),
        Ok(_) => check_access(output, libc::W_OK, "output file")
            .or_else(|file_error| directory().map_err(|_| file_error)),
    }
}

/// Checks, without compiling, that the temporary directory exists and can be written: `compile`
/// writes an object file there for each codegen unit.
fn check_temp_dir() -> Result<(), String> {
    let dir = std::env::temp_dir();
    // An empty `TMPDIR` is the current directory.
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        &dir
    };
    check_directory(dir, "temporary directory")?;
    check_access(dir, libc::W_OK | libc::X_OK, "temporary directory")
}

/// Checks that `dir` exists and is a directory. `what` names it in the message.
fn check_directory(dir: &Path, what: &str) -> Result<(), String> {
    let shown = dir.display();
    match fs::metadata(dir) {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(format!("{what} {shown} is not a directory")),
        Err(e) if e.kind() == ErrorKind::NotFound => Err(format!("{what} {shown} does not exist")),
        Err(e) => Err(format!("cannot access {what} {shown}: {e}")),
    }
}

/// Checks that the process can use `path` as `mode` says (`libc::W_OK`, `libc::X_OK`), with its
/// effective user and group IDs, not the real ones. `what` names it in the message. Search
/// permission (`X_OK`) is part of the check for a directory: without it nothing in it can be
/// created.
#[expect(
    unsafe_code,
    reason = "faccessat has no equivalent in std, and it takes the effective IDs into account"
)]
fn check_access(path: &Path, mode: libc::c_int, what: &str) -> Result<(), String> {
    let shown = path.display();
    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return Err(format!(
            "cannot write to {what} {shown}: it holds a NUL byte"
        ));
    };
    // SAFETY: `c_path` is a NUL-terminated string that lives for the whole call, and `faccessat`
    // reads it and nothing else.
    let status =
        unsafe { libc::faccessat(libc::AT_FDCWD, c_path.as_ptr(), mode, libc::AT_EACCESS) };
    if status == 0 {
        Ok(())
    } else {
        let e = io::Error::last_os_error();
        Err(format!("cannot write to {what} {shown}: {e}"))
    }
}

/// Runs the front end on `source` and reports every diagnostic. Returns the program unless a
/// diagnostic is an error.
fn front_end(
    reporter: &mut Report,
    path: &str,
    source: &str,
    timings: &mut Timings,
) -> Option<Program> {
    let (program, diagnostics) =
        front_end::front_end_timed(front_end::PRELUDE_SOURCE, source, timings);
    reporter.diagnostics(path, source, &diagnostics);
    program
}

/// Compiles `program` into the executable `output`, linked with `runtime`, going through a
/// temporary object file in `dir` for each codegen unit.
#[expect(
    clippy::too_many_arguments,
    reason = "the inputs of one compile; a struct would only rename them"
)]
fn compile(
    program: &Program,
    path: &str,
    source: &str,
    runtime: &Path,
    output: &Path,
    dir: &Path,
    backend: &Backend,
    timings: &mut Timings,
) -> Result<(), String> {
    let Backend {
        level,
        rc_checks,
        ref linker,
        allocator: _,
        units: wanted,
    } = *backend;
    let mir = timings.time(timings::Stage::Mir, || mir::build(program));
    let units = codegen::units::split(&mir, program, wanted, level);
    let results = each_unit(
        &UnitInput {
            program,
            mir: &mir,
            path,
            source,
            level,
            rc_checks,
            keep_names: false,
        },
        &units,
        timings,
        |at, module, machine, timings| {
            timings.time(timings::Stage::Optimize, || {
                codegen::optimize(module, machine, level)
            })?;
            let code = timings.time(timings::Stage::Object, || {
                codegen::object_code(module, machine)
            })?;
            // A name of its own, so a user's `<output>.o` is left alone. A panic (an internal
            // compiler error) ends `sisu` with no cleanup, under `panic = "abort"`, and can leave
            // the object of another unit here.
            create_object(dir, code.as_slice(), fresh_names(format!("-{at}.o")))
        },
    );
    release(mir);
    let objects = written_objects(results)?;
    // Remove the object files whether or not the link worked; a link error wins.
    let linking = link::Linking {
        runtime,
        output,
        linker: linker.as_deref(),
    };
    let link_result = timings.time(timings::Stage::Link, || link::link(&objects, &linking));
    let removed = remove_objects(&objects);
    link_result.and(removed)
}

/// The object files of the units, in unit order, when every unit wrote one. Else the error of the
/// lowest unit that failed, and the files that the other units wrote are removed.
fn written_objects(results: Vec<Result<PathBuf, String>>) -> Result<Vec<PathBuf>, String> {
    let mut objects = Vec::new();
    let mut first_error = None;
    for result in results {
        match result {
            Ok(object) => objects.push(object),
            Err(message) => {
                first_error.get_or_insert(message);
            }
        }
    }
    let Some(message) = first_error else {
        return Ok(objects);
    };
    Err(match remove_objects(&objects) {
        Ok(()) => message,
        Err(removal) => format!("{message}; {removal}"),
    })
}

/// Removes every file of `objects`; the error is the first that failed.
fn remove_objects(objects: &[PathBuf]) -> Result<(), String> {
    let failures: Vec<String> = (objects.iter())
        .filter_map(|object| {
            fs::remove_file(object)
                .err()
                .map(|e| format!("cannot remove {}: {e}", object.display()))
        })
        .collect();
    failures.into_iter().next().map_or(Ok(()), Err)
}

/// Ten names for a new file or directory in the temp dir: `sisu-<pid>-<nanoseconds><suffix>`.
/// The nanoseconds pick a fresh name when one is taken.
fn fresh_names(suffix: impl Into<String>) -> impl Iterator<Item = String> {
    let pid = std::process::id();
    let suffix = suffix.into();
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

    /// The IR text of a program with a call and a `let`, built as one unit through `each_unit`.
    fn printed_unit(keep_names: bool) -> String {
        let source = "fn twice(n: i64) -> i64 {\n    n * 2\n}\n\
                      fn main() {\n    let a = twice(3)\n    let s = \"x\"\n    let t = s\n    \
                      print(a)\n    print(t)\n}\n";
        let mut timings = Timings::off();
        let (program, diagnostics) =
            front_end::front_end_timed(front_end::PRELUDE_SOURCE, source, &mut timings);
        let program = program.unwrap_or_else(|| panic!("{diagnostics:?}"));
        let mir = mir::build(&program);
        let level = OptLevel::O0;
        let units = codegen::units::split(&mir, &program, Some(1), level);
        let input = UnitInput {
            program: &program,
            mir: &mir,
            path: "test.sisu",
            source,
            level,
            rc_checks: false,
            keep_names,
        };
        let mut printed = each_unit(&input, &units, &mut timings, |_, module, _, _| {
            Ok(module.print_to_string().to_string())
        });
        printed.remove(0).expect("the unit builds")
    }

    /// Proves that a unit of `sisu build` has no names for its local values, which saves LLVM the
    /// work to make and to intern them.
    #[test]
    fn build_units_should_carry_no_value_names() {
        let ir = printed_unit(false);
        assert!(!ir.contains("%call ="), "{ir}");
        assert!(!ir.contains("%moved ="), "{ir}");
    }

    /// Proves that a unit of `--emit ir` and `--emit ir-raw` keeps the names that the tests read.
    #[test]
    fn ir_units_should_keep_value_names() {
        let ir = printed_unit(true);
        assert!(ir.contains("%call ="), "{ir}");
        assert!(ir.contains("%moved ="), "{ir}");
    }

    #[test]
    fn release_should_drop_the_value_outside_the_command_line_tool() {
        let value = std::rc::Rc::new(());
        release(std::rc::Rc::clone(&value));
        assert_eq!(std::rc::Rc::strong_count(&value), 1);
    }

    #[test]
    fn written_objects_should_remove_every_object_and_keep_the_first_error() {
        let dir = std::env::temp_dir().join(format!("sisu-written-objects-{}", std::process::id()));
        // It is absent unless an earlier run failed, so a failure here is expected and harmless.
        drop(fs::remove_dir_all(&dir));
        fs::create_dir(&dir).expect("creates the directory");
        let [a, b] = ["a.o", "b.o"].map(|name| {
            let path = dir.join(name);
            fs::write(&path, "object").expect("writes");
            path
        });

        // A failed unit removes the objects of the units that worked, and the lowest error wins.
        let failed = written_objects(vec![
            Ok(a.clone()),
            Err("unit 1".to_string()),
            Ok(b.clone()),
            Err("unit 3".to_string()),
        ]);
        assert_eq!(failed, Err("unit 1".to_string()));
        assert!(!a.exists() && !b.exists());

        // Every unit worked: the paths come back in order, and the files stay.
        for path in [&a, &b] {
            fs::write(path, "object").expect("writes");
        }
        let written = written_objects(vec![Ok(a.clone()), Ok(b.clone())]);
        assert_eq!(written, Ok(vec![a.clone(), b.clone()]));
        assert!(a.exists() && b.exists());
        fs::remove_dir_all(&dir).expect("removes the directory");
    }

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
