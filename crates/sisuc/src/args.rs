//! The command line: clap's types, and the `Mode` and `Format` they become.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::builder::StyledStr;
use clap::error::ErrorKind;
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};

use crate::codegen::OptLevel;
use crate::report::Format;
use crate::{Backend, Mode, Stage};

/// The Sisu compiler.
#[derive(Parser)]
#[command(name = "sisuc")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Compile a program into an executable, or print one stage of it with `--emit`
    Build {
        #[command(flatten)]
        flags: BuildFlags,
        /// Print this stage to stdout instead of building
        #[arg(long, value_enum, conflicts_with = "output")]
        emit: Option<Stage>,
        /// The program
        file: PathBuf,
        /// The executable to write; by default the file's name without its extension, in the
        /// current directory
        #[arg(short)]
        output: Option<PathBuf>,
    },
    /// Report the diagnostics of a program without building it
    Check {
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
        /// The program
        file: PathBuf,
    },
    /// Print the explanation of a diagnostic code
    Explain { code: String },
}

/// The flags `build` takes besides its own.
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
    #[arg(long, value_enum, default_value = "text")]
    format: Format,
}

/// The format and the mode `args`, the whole command line, asks for.
pub(crate) fn parse(args: Vec<OsString>) -> Result<(Format, Mode), clap::Error> {
    let Some(command) = Cli::try_parse_from(args)?.command else {
        return Ok((Format::Text, Mode::Help));
    };
    Ok(match command {
        Command::Build {
            flags,
            emit,
            file,
            output,
        } => {
            let backend = Backend {
                level: flags.level.unwrap_or(OptLevel::O0),
                rc_checks: flags.rc_checks,
                linker: flags.linker,
            };
            (flags.format, build_mode(backend, emit, file, output)?)
        }
        Command::Check { format, file } => (format, Mode::Check { input: file }),
        Command::Explain { code } => (Format::Text, Mode::Explain { code }),
    })
}

/// The mode `build` asks for, once the flags it takes are checked against `--emit`.
fn build_mode(
    backend: Backend,
    emit: Option<Stage>,
    file: PathBuf,
    output: Option<PathBuf>,
) -> Result<Mode, clap::Error> {
    let Some(stage) = emit else {
        let output = match output {
            Some(output) => output,
            None => PathBuf::from(file.file_stem().ok_or_else(|| {
                build_error(format!(
                    "cannot name an executable after `{}`; give one with `-o <output>`",
                    file.display()
                ))
            })?),
        };
        return Ok(Mode::Build {
            input: file,
            output,
            backend,
        });
    };
    // The other stages run no optimizer.
    if backend.level != OptLevel::O0 && !matches!(stage, Stage::Ir) {
        let level = backend
            .level
            .to_possible_value()
            .expect("`-O` should take only levels it can name");
        return Err(build_error(format!(
            "`-O{}` works only when building an executable or with `--emit ir`",
            level.get_name()
        )));
    }
    if backend.rc_checks && !matches!(stage, Stage::Ir | Stage::IrRaw) {
        return Err(build_error(
            "`--rc-checks` works only when building an executable or with `--emit ir` or \
             `--emit ir-raw`",
        ));
    }
    if backend.linker.is_some() {
        return Err(build_error(
            "`--linker` works only when building an executable",
        ));
    }
    Ok(Mode::Emit {
        stage,
        input: file,
        level: backend.level,
        rc_checks: backend.rc_checks,
    })
}

/// A usage error of `build`, shown with `build`'s usage.
fn build_error(message: impl std::fmt::Display) -> clap::Error {
    let mut cli = Cli::command();
    // Gives the subcommand its full name, `sisuc build`, for the usage line.
    cli.build();
    cli.find_subcommand_mut("build")
        .expect("`Cli` should have the subcommand `build`")
        .error(ErrorKind::ArgumentConflict, message)
}

/// The help that `sisuc` with no arguments prints, as `sisuc --help` does.
pub(crate) fn help() -> StyledStr {
    Cli::command().render_help()
}
