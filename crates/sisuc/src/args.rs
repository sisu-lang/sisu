//! The command line: clap's types, and the `Mode` and `Format` they become.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::builder::StyledStr;
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{ArgGroup, Args, CommandFactory, Parser, Subcommand, ValueEnum};

use crate::codegen::OptLevel;
use crate::report::Format;
use crate::{Allocator, Backend, Mode, Source, Stage};

/// The Sisu compiler.
#[derive(Parser)]
#[command(
    name = "sisu",
    args_conflicts_with_subcommands = true,
    // clap's own `--version` would exit before `-v` is seen, so `-vV` could not work.
    disable_version_flag = true
)]
struct Cli {
    /// Print the version
    #[arg(short = 'V', long)]
    version: bool,
    /// With `--version`, also print the commit, the host and the LLVM version
    #[arg(short, long, requires = "version")]
    verbose: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Compile a program into an executable, or print one output of it with `--emit`
    Build {
        #[command(flatten)]
        flags: BuildFlags,
        /// Print this output of the compiler to stdout instead of building
        #[arg(long, value_enum, conflicts_with = "output")]
        emit: Option<Stage>,
        #[command(flatten)]
        program: Program,
        /// The executable to write; by default the file's name without its extension, in the
        /// current directory
        #[arg(short)]
        output: Option<PathBuf>,
        /// Print the time of each stage to stderr
        #[arg(long)]
        timings: bool,
    },
    /// Compile a program and run it; exit with its exit code
    #[command(group = ArgGroup::new("program").required(true).args(["inline", "rest"]))]
    Run {
        #[command(flatten)]
        flags: BuildFlags,
        /// The program, as text on the command line, then the program's arguments
        #[arg(short = 'e', value_name = "CODE", num_args = 1.., allow_hyphen_values = true)]
        inline: Vec<OsString>,
        /// The program, then its arguments
        #[arg(value_name = "FILE", trailing_var_arg = true, num_args = 1..)]
        rest: Vec<OsString>,
    },
    /// Report the diagnostics of a program without building it
    Check {
        /// How to print diagnostics: as text, or as JSON Lines.
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
        /// Print the time of each stage to stderr
        #[arg(long)]
        timings: bool,
        #[command(flatten)]
        program: Program,
    },
    /// Print the explanation of a diagnostic code
    Explain {
        /// A diagnostic code, such as E0001.
        code: String,
    },
}

/// The program `build` and `check` take: a file, or the text of one with `-e`.
#[derive(Args)]
#[group(required = true, multiple = false)]
struct Program {
    /// The program
    file: Option<PathBuf>,
    /// The program, as text on the command line
    #[arg(short = 'e', value_name = "CODE")]
    inline: Option<OsString>,
}

impl Program {
    fn into_source(self) -> Source {
        self.file
            .map(Source::File)
            .or_else(|| self.inline.map(Source::Inline))
            .expect("clap should require a file or `-e`")
    }
}

/// The flags `build` and `run` take besides their own.
#[derive(Args)]
struct BuildFlags {
    /// Optimize at this level
    #[arg(short = 'O', value_enum)]
    level: Option<OptLevel>,
    /// Build in checks of every reference count
    #[arg(long)]
    rc_checks: bool,
    /// Link with `ld.<linker>` in place of `cc`'s default linker
    #[arg(long, value_parser = ["bfd", "gold", "lld", "mold"])]
    linker: Option<String>,
    /// Allocate with mimalloc, or with the system's allocator for Valgrind and ASan
    #[expect(
        clippy::doc_markdown,
        reason = "this doc comment is `--help` text, where backticks around ASan would print"
    )]
    #[arg(long, value_enum)]
    allocator: Option<Allocator>,
    /// How to print diagnostics: as text, or as JSON Lines.
    #[arg(long, value_enum, default_value = "text")]
    format: Format,
}

impl BuildFlags {
    /// The format, and the back end these flags ask for.
    fn into_parts(self) -> (Format, Backend) {
        let backend = Backend {
            level: self.level.unwrap_or(OptLevel::O0),
            rc_checks: self.rc_checks,
            linker: self.linker,
            allocator: self.allocator,
        };
        (self.format, backend)
    }
}

/// The format and the mode `args`, the whole command line, asks for.
pub(crate) fn parse(args: &[OsString]) -> Result<(Format, Mode), clap::Error> {
    let cli = Cli::try_parse_from(args).map_err(|error| foreign_flag(args, error))?;
    if cli.version {
        return Ok((
            Format::Text,
            Mode::Version {
                verbose: cli.verbose,
            },
        ));
    }
    let Some(command) = cli.command else {
        return Ok((Format::Text, Mode::Help));
    };
    Ok(match command {
        Command::Build {
            flags,
            emit,
            program,
            output,
            timings,
        } => {
            let (format, backend) = flags.into_parts();
            reject_json_timings(&format, timings, "build")?;
            (
                format,
                build_mode(backend, emit, program.into_source(), output, timings)?,
            )
        }
        Command::Run {
            flags,
            inline,
            rest,
        } => {
            let (format, backend) = flags.into_parts();
            let inline_given = !inline.is_empty();
            let mut values = if inline_given { inline } else { rest }
                .into_iter()
                .peekable();
            let first = values
                .next()
                .expect("clap should require a file or `-e` with code");
            let source = if inline_given {
                Source::Inline(first)
            } else {
                Source::File(first.into())
            };
            // A first `--` only ends `sisu`'s own arguments; a later one is the program's.
            values.next_if(|arg| arg == "--");
            let args = values.collect();
            (
                format,
                Mode::Run {
                    source,
                    args,
                    backend,
                },
            )
        }
        Command::Check {
            format,
            timings,
            program,
        } => {
            reject_json_timings(&format, timings, "check")?;
            (
                format,
                Mode::Check {
                    source: program.into_source(),
                    timings,
                },
            )
        }
        Command::Explain { code } => (Format::Text, Mode::Explain { code }),
    })
}

/// A usage error of `subcommand` for `--timings` with `--format json`: a text line would break
/// the JSON Lines stream.
fn reject_json_timings(
    format: &Format,
    timings: bool,
    subcommand: &str,
) -> Result<(), clap::Error> {
    if timings && matches!(format, Format::Json) {
        return Err(usage_error(
            subcommand,
            "`--timings` does not work with `--format json`",
        ));
    }
    Ok(())
}

/// The mode `build` asks for, once the flags it takes are checked against `--emit`.
fn build_mode(
    backend: Backend,
    emit: Option<Stage>,
    source: Source,
    output: Option<PathBuf>,
    timings: bool,
) -> Result<Mode, clap::Error> {
    let Some(stage) = emit else {
        let output = match (output, &source) {
            (Some(output), _) => output,
            (None, Source::File(file)) => PathBuf::from(file.file_stem().ok_or_else(|| {
                usage_error(
                    "build",
                    format!(
                        "cannot name an executable after `{}`; give one with `-o <output>`",
                        file.display()
                    ),
                )
            })?),
            (None, Source::Inline(_)) => {
                return Err(usage_error(
                    "build",
                    "an inline program needs `-o <output>` to build an executable",
                ));
            }
        };
        return Ok(Mode::Build {
            input: source,
            output,
            backend,
            timings,
        });
    };
    // The other stages run no optimizer.
    if backend.level != OptLevel::O0 && !matches!(stage, Stage::Ir) {
        let level = backend
            .level
            .to_possible_value()
            .expect("`-O` should take only levels it can name");
        return Err(usage_error(
            "build",
            format!(
                "`-O{}` works only when building an executable or with `--emit ir`",
                level.get_name()
            ),
        ));
    }
    if backend.rc_checks && !matches!(stage, Stage::Ir | Stage::IrRaw) {
        return Err(usage_error(
            "build",
            "`--rc-checks` works only when building an executable or with `--emit ir` or \
             `--emit ir-raw`",
        ));
    }
    if backend.linker.is_some() {
        return Err(usage_error(
            "build",
            "`--linker` works only when building an executable",
        ));
    }
    if backend.allocator.is_some() {
        return Err(usage_error(
            "build",
            "`--allocator` works only when building an executable",
        ));
    }
    Ok(Mode::Emit {
        stage,
        input: source,
        level: backend.level,
        rc_checks: backend.rc_checks,
        timings,
    })
}

/// `error` as a usage error of the subcommand on `args`, when `error` is an unknown flag that
/// other subcommands take; else `error`, which clap's own text explains.
fn foreign_flag(args: &[OsString], error: clap::Error) -> clap::Error {
    // clap gives an unknown argument its flag alone, as a string.
    let (ErrorKind::UnknownArgument, Some(ContextValue::String(flag))) =
        (error.kind(), error.get(ContextKind::InvalidArg))
    else {
        return error;
    };
    let cli = Cli::command();
    // The flag alone is `--long` or `-s`.
    let takes = |arg: &clap::Arg| {
        arg.get_long()
            .is_some_and(|long| flag.strip_prefix("--") == Some(long))
            || arg
                .get_short()
                .is_some_and(|short| *flag == format!("-{short}"))
    };
    // After a `--`, the flag is a value, which every command takes.
    let flags = || {
        args.iter()
            .skip(1)
            .take_while(|arg| arg.as_os_str() != "--")
    };
    if !flags().any(|arg| {
        arg.to_str()
            .is_some_and(|arg| arg.starts_with(flag.as_str()))
    }) {
        return error;
    }
    let Some(used) = flags().find_map(|arg| cli.find_subcommand(arg)) else {
        return error;
    };
    // A flag before the subcommand's name, such as `sisu --timings build`, is not its to reject.
    if used.get_arguments().any(takes) {
        return error;
    }
    let others: Vec<String> = cli
        .get_subcommands()
        .filter(|sub| sub.get_arguments().any(takes))
        .map(|sub| format!("`sisu {}`", sub.get_name()))
        .collect();
    let Some((last, first)) = others.split_last() else {
        return error;
    };
    let (names, verb) = if first.is_empty() {
        (last.clone(), "takes")
    } else {
        (format!("{} and {last}", first.join(", ")), "take")
    };
    usage_error(
        used.get_name(),
        format!(
            "`sisu {}` does not take `{flag}`; {names} {verb} it",
            used.get_name()
        ),
    )
}

/// A usage error of `subcommand`, shown with its usage.
fn usage_error(subcommand: &str, message: impl std::fmt::Display) -> clap::Error {
    let mut cli = Cli::command();
    // Gives the subcommand its full name, such as `sisu build`, for the usage line.
    cli.build();
    cli.find_subcommand_mut(subcommand)
        .expect("`Cli` should have the subcommand that the caller names")
        .error(ErrorKind::ArgumentConflict, message)
}

/// The help that `sisu` with no arguments prints, as `sisu --help` does.
pub(crate) fn help() -> StyledStr {
    Cli::command().render_help()
}

/// The first line of `--version`: the command's name and the version.
pub(crate) fn version() -> String {
    format!(
        "{} {}",
        Cli::command().get_name(),
        env!("CARGO_PKG_VERSION")
    )
}
