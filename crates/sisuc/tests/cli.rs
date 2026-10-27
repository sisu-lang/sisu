//! End to end: the `sisu` command line.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

mod scratch_path;

/// Builds `source`, saved as `<name>.sisu`, with `--emit tokens`. Returns stdout when the exit code
/// is 0.
fn emit_tokens(name: &str, source: &str) -> String {
    let out = run_on(name, source, &["build", "--emit", "tokens"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// One test for each case, so nextest runs them in parallel. `exact` compares all of stdout and
/// `prefix` compares only its start. The source file is named after the test.
macro_rules! emit_tokens_cases {
    (@check exact $stdout:expr, $expected:expr) => {
        assert_eq!($stdout, $expected);
    };
    (@check prefix $stdout:expr, $expected:expr) => {
        assert!($stdout.starts_with($expected), "{}", $stdout);
    };
    ($($(#[$meta:meta])* $name:ident($source:expr) $kind:ident $expected:expr;)*) => {
        $(
            $(#[$meta])*
            #[test]
            fn $name() {
                let stdout = emit_tokens(stringify!($name), $source);
                emit_tokens_cases!(@check $kind stdout, $expected);
            }
        )*
    };
}

emit_tokens_cases! {
    /// GIVEN a file that holds `fn main() {}`
    /// WHEN it is built with `--emit tokens`
    /// THEN the output starts with the `Fn` token at 1:1 and the `Ident` token at 1:4
    emit_tokens_should_print_each_token_with_its_line_and_column("fn main() {}\n")
        prefix "1:1 Fn fn\n1:4 Ident main\n";
    /// GIVEN a source with the character literals `'ä'` and `'\n'`
    /// WHEN it is built with `--emit tokens`
    /// THEN the output starts with two `Char` tokens, at 1:1 and at 1:5, that show each literal
    emit_tokens_should_print_a_char_token_for_each_character_literal("'ä' '\\n'\n")
        prefix "1:1 Char 'ä'\n1:5 Char '\\n'\n";
    /// GIVEN a source that holds only the literal `1.5e3`
    /// WHEN it is built with `--emit tokens`
    /// THEN the output is one `Float` token, then `Newline` and `Eof` at 1:6
    emit_tokens_should_print_a_float_token_for_a_literal_with_an_exponent("1.5e3\n")
        exact "1:1 Float 1.5e3\n1:6 Newline\n1:6 Eof\n";
    /// GIVEN a source that holds `x ** 2`
    /// WHEN it is built with `--emit tokens`
    /// THEN the output has one `StarStar` token between the `Ident` and the `Int` tokens
    emit_tokens_should_print_a_star_star_token_for_the_power_operator("x ** 2\n")
        exact "1:1 Ident x\n1:3 StarStar **\n1:6 Int 2\n1:7 Newline\n1:7 Eof\n";
    /// GIVEN a source that holds only `todo`
    /// WHEN it is built with `--emit tokens`
    /// THEN the output is a `Todo` token, then `Newline` and `Eof`
    emit_tokens_should_print_a_todo_token_for_the_todo_keyword("todo\n")
        exact "1:1 Todo todo\n1:5 Newline\n1:5 Eof\n";
    /// GIVEN a source that prints the string `"{x:>8.2}"`
    /// WHEN it is built with `--emit tokens`
    /// THEN the output has the tokens `StrHead`, `Ident`, `FormatSpec` with `:>8.2`, and `StrTail`
    emit_tokens_should_print_a_format_spec_token_inside_an_interpolated_string("print(\"{x:>8.2}\")\n")
        exact "1:1 Ident print\n1:6 LParen (\n1:7 StrHead \"{\n1:9 Ident x\n1:10 FormatSpec :>8.2\n1:15 StrTail }\"\n1:17 RParen )\n1:18 Newline\n1:18 Eof\n";
    /// GIVEN a source that holds only `enum`
    /// WHEN it is built with `--emit tokens`
    /// THEN the output is an `Enum` token and then `Eof`
    emit_tokens_should_print_an_enum_token_for_the_enum_keyword("enum\n")
        exact "1:1 Enum enum\n1:5 Eof\n";
    /// GIVEN a source that holds `match =>`
    /// WHEN it is built with `--emit tokens`
    /// THEN the output is a `Match` token, a `FatArrow` token and `Eof`
    emit_tokens_should_print_match_and_fat_arrow_tokens("match =>\n")
        exact "1:1 Match match\n1:7 FatArrow =>\n1:9 Eof\n";
    /// GIVEN a source that holds `| ..= ||`
    /// WHEN it is built with `--emit tokens`
    /// THEN the output is the tokens `Pipe`, `DotDotEq` and `OrOr`, then `Eof`
    emit_tokens_should_print_pipe_inclusive_range_and_or_or_tokens("| ..= ||\n")
        exact "1:1 Pipe |\n1:3 DotDotEq ..=\n1:7 OrOr ||\n1:9 Eof\n";
}

/// GIVEN a source with `class`, `None`, `is`, `self`, `.`, `?`, `?.`, `??` and `_`
/// WHEN it is built with `--emit tokens`
/// THEN the first nine token names are `Class`, `NoneKw`, `Is`, `SelfKw`, `Dot`, `Question`,
/// `QuestionDot`, `QuestionQuestion` and `Underscore`
#[test]
fn emit_tokens_should_name_the_class_none_and_optional_chaining_tokens() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(
        dir.join("emit_new_tokens.sisu"),
        "class None is self . ? ?. ?? _",
    )
    .expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["build", "--emit", "tokens", "emit_new_tokens.sisu"])
        .output()
        .expect("sisu starts");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let names: Vec<&str> = stdout
        .lines()
        .map(|l| l.split(' ').nth(1).expect("name column"))
        .collect();
    assert_eq!(
        names[..9],
        [
            "Class",
            "NoneKw",
            "Is",
            "SelfKw",
            "Dot",
            "Question",
            "QuestionDot",
            "QuestionQuestion",
            "Underscore"
        ]
    );
}

/// GIVEN a source with a character literal, a string with two holes and a plain string
/// WHEN it is built with `--emit tokens`
/// THEN the output has `StrHead`, `StrMiddle` and `StrTail` tokens around the holes, and a `Str`
/// token
/// for the plain string
#[test]
fn emit_tokens_should_split_an_interpolated_string_into_head_middle_and_tail_tokens() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(
        dir.join("emit_tokens_strings.sisu"),
        "let c = 'a'\nprint(\"a{c}b{c}c\" + \"d\")\n",
    )
    .expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["build", "--emit", "tokens", "emit_tokens_strings.sisu"])
        .output()
        .expect("sisu starts");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in [
        "1:9 Char 'a'\n",
        "2:7 StrHead \"a{\n2:10 Ident c\n2:11 StrMiddle }b{\n2:14 Ident c\n2:15 StrTail }c\"\n",
        "2:21 Str \"d\"\n",
    ] {
        assert!(stdout.contains(line), "{stdout}");
    }
}

/// GIVEN a source with the words `record` and `const` on two lines
/// WHEN it is built with `--emit tokens`
/// THEN `record` is a `Reserved` token at 1:1 and `const` is a `Const` token at 2:1
#[test]
fn emit_tokens_should_print_reserved_and_const_words_as_their_own_tokens() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("emit_tokens_reserved.sisu"), "record\nconst\n").expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["build", "--emit", "tokens", "emit_tokens_reserved.sisu"])
        .output()
        .expect("sisu starts");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("1:1 Reserved record\n"), "{stdout}");
    assert!(stdout.contains("\n2:1 Const const\n"), "{stdout}");
}

/// GIVEN a source that holds `a & b`
/// WHEN it is built with `--emit tokens`
/// THEN the exit code is 1, the first diagnostic is an error, and it is E0010 at 1:3 of the file
#[test]
fn emit_tokens_should_exit_1_with_e0010_when_the_source_has_an_unknown_character() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("lexer_error_exits_1.sisu"), "a & b\n").expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["build", "--emit", "tokens", "lexer_error_exits_1.sisu"])
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(at_severity(&stderr, "error") == Some(0), "{stderr}");
    assert!(
        stderr
            .contains("error[E0010]: unexpected character `&`\n --> lexer_error_exits_1.sisu:1:3"),
        "{stderr}"
    );
}

/// GIVEN a source that holds a stray bracket
/// WHEN it is built with `--emit tokens` and with `--emit ast`
/// THEN each run exits 1 and the first diagnostic on stderr is an error
///
/// A source the lexer cannot finish (a stray bracket) fails `--emit tokens` and `--emit ast`.
#[test]
fn emit_tokens_and_ast_should_exit_1_when_the_lexer_cannot_finish_the_source() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("unlexable_source.sisu"), ")\n").expect("writes");
    for stage in ["tokens", "ast"] {
        let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
            .current_dir(dir)
            .args(["build", "--emit", stage, "unlexable_source.sisu"])
            .output()
            .expect("sisu starts");
        assert_eq!(out.status.code(), Some(1), "{stage}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            at_severity(&stderr, "error") == Some(0),
            "{stage}: {stderr}"
        );
    }
}

/// GIVEN no arguments, `help`, `-h` or `--help`
/// WHEN `sisu` runs
/// THEN it exits 0, stdout names `build`, `run`, `check` and `explain`, and stderr is empty
///
/// No arguments, `help`, `-h` and `--help` print the usage to stdout and exit 0.
#[test]
fn usage_should_print_to_stdout_and_exit_0_when_help_is_asked_for() {
    for args in [&[][..], &["help"], &["-h"], &["--help"]] {
        let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
            .args(args)
            .output()
            .expect("sisu starts");
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        for command in ["build", "run", "check", "explain"] {
            assert!(stdout.contains(command), "{args:?}: {stdout}");
        }
        assert_eq!(String::from_utf8_lossy(&out.stderr), "", "{args:?}");
    }
}

/// GIVEN a stdout that is `/dev/full`
/// WHEN `sisu` prints help, the version, an explanation or any emitted stage
/// THEN it exits 1 and stderr says it cannot write to stdout because no space is left
///
/// Help, version, an explanation and an emitted stage that cannot be written are a tool error,
/// not a panic or a silent success.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "writes to /dev/full, which only Linux has"
)]
fn sisu_should_exit_1_with_a_tool_error_when_stdout_is_full() {
    const MAIN: &str = "fn main() { print(1) }";
    for args in [
        &[][..],
        &["--help"],
        &["help"],
        &["build", "--help"],
        &["--version"],
        &["explain", "E0001"],
        &["build", "--emit", "tokens", "-e", MAIN],
        &["build", "--emit", "ast", "-e", MAIN],
        &["build", "--emit", "tir", "-e", MAIN],
        &["build", "--emit", "mir", "-e", MAIN],
        &["build", "--emit", "ir", "-e", MAIN],
        &["build", "--emit", "ir-raw", "-e", MAIN],
    ] {
        let full = fs::File::options()
            .write(true)
            .open("/dev/full")
            .expect("/dev/full exists on Linux");
        let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
            .args(args)
            .stdout(full)
            .output()
            .expect("sisu starts");
        assert_eq!(out.status.code(), Some(1), "{args:?}: {:?}", out.status);
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            "sisu: cannot write to stdout: No space left on device (os error 28)\n",
            "{args:?}"
        );
    }
}

/// GIVEN a stdout pipe whose read end is closed
/// WHEN `sisu` prints `--help` or an explanation
/// THEN it exits 0 and stderr is empty
///
/// A reader that has already left is a quiet end, not an error: `sisu --help | head -1`. The read
/// end is closed before `sisu` starts, so the write fails every time.
#[test]
fn sisu_should_exit_0_and_print_no_error_when_stdout_is_a_closed_pipe() {
    // `--help` is written by clap in several writes, `explain` by `print_stdout`.
    for args in [&["--help"][..], &["explain", "E0001"]] {
        let (reader, writer) = std::io::pipe().expect("a pipe should open");
        drop(reader);
        let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
            .args(args)
            .stdout(writer)
            .stderr(Stdio::piped())
            .output()
            .expect("sisu starts");
        assert_eq!(out.status.code(), Some(0), "{args:?}: {:?}", out.status);
        assert_eq!(String::from_utf8_lossy(&out.stderr), "", "{args:?}");
    }
}

/// GIVEN an unknown command, a missing argument, a repeated file or a flag the command does not
/// allow
/// WHEN `sisu` runs
/// THEN it exits 2 with clap's error on stderr and nothing on stdout
/// AND the error for `-O2` with `--emit tir` names the flag and shows the usage of `build`
///
/// A wrong command line exits 2, and a broken flag rule names the flag with `build`'s usage.
#[test]
fn sisu_should_exit_2_with_a_usage_error_when_the_command_line_is_wrong() {
    assert_usage_errors(&[
        &["bogus"],
        &["build"],
        &["explain"],
        &["build", "a.sisu", "b.sisu"],
        &["check", "--emit", "ast", "a.sisu"],
        &["build", ".."],
    ]);
    let stderr = assert_usage_errors(&[&["build", "--emit", "tir", "-O2", "f.sisu"]]);
    assert!(
        stderr.starts_with(
            "error: `-O2` works only when building an executable or with `--emit ir`\n"
        ),
        "{stderr}"
    );
    assert!(stderr.contains("\nUsage: sisu build "), "{stderr}");
}

/// GIVEN `check` with no file, or with the arguments of another command
/// WHEN `sisu` runs
/// THEN it exits 2 with a usage error
///
/// A subcommand with too few arguments, or with the arguments of another one.
#[test]
fn check_should_exit_2_when_it_has_no_file_or_gets_the_arguments_of_another_command() {
    assert_usage_errors(&[&["check"], &["check", "tir", "f.sisu"]]);
}

/// GIVEN the flag `--version` or `-V`
/// WHEN `sisu` runs
/// THEN it exits 0, stdout is `sisu` and the package version, and stderr is empty
///
/// `--version` and `-V` print the name and the version, and nothing else.
#[test]
fn version_flags_should_print_the_name_and_the_version_and_nothing_else() {
    for flag in ["--version", "-V"] {
        let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
            .arg(flag)
            .output()
            .expect("sisu starts");
        assert_eq!(out.status.code(), Some(0), "{flag}");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            concat!("sisu ", env!("CARGO_PKG_VERSION"), "\n"),
            "{flag}"
        );
        assert_eq!(String::from_utf8_lossy(&out.stderr), "", "{flag}");
    }
}

/// GIVEN the flags `--version --verbose` or `-vV`
/// WHEN `sisu` runs
/// THEN it exits 0 and prints four lines: the name and version, the commit, the host and the LLVM
/// version
///
/// `--version --verbose` adds the commit, the host and the LLVM version; each line is checked
/// by shape, since the values change with the checkout and the machine.
#[test]
fn verbose_version_should_print_name_commit_host_and_llvm_version_on_four_lines() {
    for args in [&["--version", "--verbose"][..], &["-vV"]] {
        let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
            .args(args)
            .output()
            .expect("sisu starts");
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines.len(), 4, "{args:?}: {stdout}");
        assert_eq!(
            lines[0],
            concat!("sisu ", env!("CARGO_PKG_VERSION")),
            "{args:?}"
        );
        let commit = lines[1]
            .strip_prefix("commit: ")
            .unwrap_or_else(|| panic!("{args:?}: {stdout}"));
        assert!(
            commit == "unknown"
                || (commit.len() >= 7 && commit.bytes().all(|b| b.is_ascii_hexdigit())),
            "{args:?}: {stdout}"
        );
        let host = lines[2]
            .strip_prefix("host: ")
            .unwrap_or_else(|| panic!("{args:?}: {stdout}"));
        assert!(host.matches('-').count() >= 2, "{args:?}: {stdout}");
        let llvm = lines[3]
            .strip_prefix("llvm: 23.")
            .unwrap_or_else(|| panic!("{args:?}: {stdout}"));
        let numbers: Vec<&str> = llvm.split('.').collect();
        assert!(
            numbers.len() == 2
                && numbers
                    .iter()
                    .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())),
            "{args:?}: {stdout}"
        );
    }
}

/// GIVEN `-v` alone, with `check`, or `-V` with a subcommand
/// WHEN `sisu` runs
/// THEN it exits 2 with a usage error
///
/// `--verbose` means something only with `--version`; `--version` takes no subcommand.
#[test]
fn verbose_should_exit_2_when_it_does_not_go_with_version() {
    assert_usage_errors(&[
        &["-v"],
        &["check", "-v", "f.sisu"],
        &["-V", "build", "a.sisu"],
    ]);
}

/// Runs `sisu` with each of `cases` in the target tmp dir, and checks that each is a usage
/// error: clap's error on stderr, nothing on stdout, and exit 2. Returns the last stderr.
fn assert_usage_errors(cases: &[&[&str]]) -> String {
    let mut stderr = String::new();
    for args in cases {
        let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
            .current_dir(env!("CARGO_TARGET_TMPDIR"))
            .args(*args)
            .output()
            .expect("sisu starts");
        stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(2), "{args:?}: {stderr}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "", "{args:?}");
        assert!(stderr.starts_with("error: "), "{args:?}: {stderr}");
    }
    stderr
}

/// GIVEN `--rc-checks` with `explain`, `check`, a stage other than `ir`, no file or a repeat
/// WHEN `sisu` runs
/// THEN it exits 2 with a usage error
/// AND the error for `--emit tir` names `--rc-checks` and the commands that allow it
///
/// `--rc-checks` goes only with a build or IR, once.
#[test]
fn rc_checks_should_exit_2_when_the_command_cannot_use_it() {
    let cases: [&[&str]; 8] = [
        &["explain", "--rc-checks", "E0001"],
        &["build", "--rc-checks", "--emit", "tir", "f.sisu"],
        &["build", "--rc-checks", "--emit", "mir", "f.sisu"],
        &["build", "--rc-checks", "--emit", "tokens", "f.sisu"],
        &["check", "--rc-checks", "f.sisu"],
        &["build", "--emit", "ir", "--rc-checks"],
        &["check", "--rc-checks"],
        &["build", "--rc-checks", "--rc-checks", "f.sisu"],
    ];
    assert_usage_errors(&cases);
    let stderr = assert_usage_errors(&[&["build", "--rc-checks", "--emit", "tir", "f.sisu"]]);
    assert!(
        stderr.starts_with(
            "error: `--rc-checks` works only when building an executable or with `--emit ir` or \
             `--emit ir-raw`\n"
        ),
        "{stderr}"
    );
}

/// GIVEN a program that prints 1
/// WHEN it is built with `--emit ir-raw` or `--emit ir`, with and without `--rc-checks`
/// THEN the IR calls `@sisu_rc_check_exit` with the flag, and does not call it without the flag
///
/// `--rc-checks` with `--emit ir-raw` and `--emit ir` prints the module with the count checks
/// built in, so a refactor can compare that IR too; without it, the module has none.
#[test]
fn emit_ir_should_contain_the_count_check_exit_function_only_with_rc_checks() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("emit_rc.sisu"), "fn main() {\n    print(1)\n}\n").expect("writes");
    let ir = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
            .current_dir(dir)
            .args(args)
            .arg("emit_rc.sisu")
            .output()
            .expect("sisu starts");
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    for args in [
        &["build", "--rc-checks", "--emit", "ir-raw"][..],
        &["build", "--rc-checks", "--emit", "ir"],
        &["build", "-O2", "--rc-checks", "--emit", "ir"],
    ] {
        assert!(ir(args).contains("@sisu_rc_check_exit"), "{args:?}");
    }
    assert!(!ir(&["build", "--emit", "ir-raw"]).contains("@sisu_rc_check_exit"));
}

/// GIVEN a program whose `main` prints `1 + 2`
/// WHEN it is built with `--emit ast`
/// THEN stdout is `(fn main () unit (block (call print (+ 1 2))))`
#[test]
fn emit_ast_should_print_each_function_as_an_s_expression() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(
        dir.join("emit_ast.sisu"),
        "fn main() {\n    print(1 + 2)\n}\n",
    )
    .expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["build", "--emit", "ast", "emit_ast.sisu"])
        .output()
        .expect("sisu starts");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "(fn main () unit (block (call print (+ 1 2))))\n"
    );
}

/// GIVEN a program that runs `x += 2` on a mutable variable
/// WHEN it is built with `--emit ast`
/// THEN the output has the node `(+= x 2)` and no `=` node
#[test]
fn emit_ast_should_keep_a_compound_assignment_as_one_node() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(
        dir.join("emit_ast_compound.sisu"),
        "fn main() {\n    let mut x = 1\n    x += 2\n}\n",
    )
    .expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["build", "--emit", "ast", "emit_ast_compound.sisu"])
        .output()
        .expect("sisu starts");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "(fn main () unit (block (let mut x 1) (+= x 2)))\n"
    );
}

/// GIVEN a class with a `mut` field and a `main` with a `let mut` variable
/// WHEN it is built with `--emit ast` and with `--emit tir`
/// THEN both outputs print `mut` for the field and the variable, and the TIR numbers the variable
/// as `x#0`
#[test]
fn emit_ast_and_tir_should_print_mut_for_mutable_variables_and_fields() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(
        dir.join("emit_mut.sisu"),
        "class _P {\n    name: i64\n    mut hp: i64\n}\nfn main() {\n    let mut x = 1\n    x = 2\n    print(x)\n}\n",
    )
    .expect("writes");
    let emit = |what: &str| {
        let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
            .current_dir(dir)
            .args(["build", "--emit", what, "emit_mut.sisu"])
            .output()
            .expect("sisu starts");
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert_eq!(
        emit("ast"),
        "(class _P (name i64) (mut hp i64))\n(fn main () unit (block (let mut x 1) (= x 2) (call print x)))\n"
    );
    assert_eq!(
        emit("tir"),
        "(class _P (name i64) (mut hp i64))\n(fn main () unit (block (let mut x#0 1) (= x#0 2) (print x#0)))\n"
    );
}

/// GIVEN a source that holds `let x = 1` at the top level
/// WHEN it is built with `--emit ast`
/// THEN the exit code is 1 and the first diagnostic is the error E0011 at 1:1 of the file
#[test]
fn emit_ast_should_exit_1_with_e0011_when_a_top_level_item_is_a_let() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("parse_error_exits_1.sisu"), "let x = 1\n").expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["build", "--emit", "ast", "parse_error_exits_1.sisu"])
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(at_severity(&stderr, "error") == Some(0), "{stderr}");
    assert!(
        stderr.contains(
            "error[E0011]: expected `fn`, `class`, `const` or `enum`, found `let`\n --> parse_error_exits_1.sisu:1:1"
        ),
        "{stderr}"
    );
}

/// The offset of the first diagnostic header of `severity` in `stderr`, with or without a code.
fn at_severity(stderr: &str, severity: &str) -> Option<usize> {
    [format!("{severity}["), format!("{severity}:")]
        .iter()
        .filter_map(|header| stderr.find(header.as_str()))
        .min()
}

/// Runs `sisu <flags> <name>.sisu` on `source`, saved as `<name>.sisu` in the target tmp dir.
fn run_on(name: &str, source: &str, flags: &[&str]) -> Output {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let file = format!("{name}.sisu");
    fs::write(dir.join(&file), source).expect("writes");
    Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(flags)
        .arg(&file)
        .output()
        .expect("sisu starts")
}

/// Runs `sisu check` on `source`, saved as `<name>.sisu` in the target tmp dir.
fn check(name: &str, source: &str) -> Output {
    run_on(name, source, &["check"])
}

/// GIVEN a valid file
/// WHEN `sisu check` runs on it
/// THEN it exits 0 and stderr is empty
#[test]
fn check_should_exit_0_with_no_output_when_the_file_is_valid() {
    let out = check("check_valid_file", "fn main() {\n    print(1)\n}\n");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
}

/// GIVEN a file with a `let mut` variable that is never reassigned
/// WHEN `sisu check` runs on it
/// THEN it exits 0 and stderr starts with the warning W0002
#[test]
fn check_should_exit_0_and_print_the_warning_when_the_file_has_only_a_warning() {
    let out = check(
        "check_warning_exits_0",
        "fn main() {\n    let mut count = 0\n    print(count)\n}\n",
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("warning[W0002]: "));
}

/// GIVEN a file that assigns to an immutable variable
/// WHEN `sisu check` runs on it
/// THEN it exits 1 and stderr starts with the error E0001
#[test]
fn check_should_exit_1_and_print_the_error_when_the_file_has_an_error() {
    let out = check(
        "check_error_exits_1",
        "fn main() {\n    let n = 0\n    n = 1\n}\n",
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("error[E0001]: cannot assign to `n`"));
}

/// GIVEN a file with an unused `let mut` and an unknown name
/// WHEN `sisu check` runs on it
/// THEN it exits 1 and the warning comes before the error on stderr
#[test]
fn check_should_print_the_warning_before_the_error_and_exit_1_when_the_file_has_both() {
    let out = check(
        "check_warning_and_error_exits_1",
        "fn f() { let mut x = 0 }\nfn main() { print(missing) }\n",
    );
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let warning = at_severity(&stderr, "warning").expect("a warning");
    let error = at_severity(&stderr, "error").expect("an error");
    assert!(warning < error, "{stderr}");
}

/// GIVEN the code `E0001` written as `E0001` and as `e0001`
/// WHEN `sisu explain` runs
/// THEN it exits 0, stdout is the page `docs/diagnostics/E0001.md`, and stderr is empty
#[test]
fn explain_should_print_the_page_of_a_code_in_either_case() {
    let expected = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/diagnostics/E0001.md"
    ))
    .expect("reads the explanation");
    for code in ["E0001", "e0001"] {
        let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
            .args(["explain", code])
            .output()
            .expect("sisu starts");
        assert_eq!(out.status.code(), Some(0), "{code}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), expected, "{code}");
        assert_eq!(String::from_utf8_lossy(&out.stderr), "", "{code}");
    }
}

/// GIVEN the retired codes `E0012` and `E0014`, each written in upper and in lower case
/// WHEN `sisu explain` runs
/// THEN it exits 0 and stdout has a line that starts with `Status: retired`
///
/// A retired code's page is still served, in either case, and says it is retired.
#[test]
fn explain_should_print_the_page_of_a_retired_code_with_its_status() {
    for code in ["E0012", "e0012", "E0014", "e0014"] {
        let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
            .args(["explain", code])
            .output()
            .expect("sisu starts");
        assert_eq!(out.status.code(), Some(0), "{code}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.lines().any(|l| l.starts_with("Status: retired")),
            "{code}: {stdout}"
        );
    }
}

/// GIVEN the code `E9999`, which does not exist
/// WHEN `sisu explain` runs
/// THEN it exits 1, stdout is empty, and stderr says the code is unknown
#[test]
fn explain_should_exit_1_with_an_error_when_the_code_is_unknown() {
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .args(["explain", "E9999"])
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "sisu: unknown diagnostic code `E9999`\n"
    );
}

/// GIVEN a path whose directory does not exist
/// WHEN `sisu check` and `sisu run` run on it
/// THEN each exits 1 and stderr says `sisu: cannot read` with the path
#[test]
fn check_and_run_should_exit_1_when_the_input_file_cannot_be_read() {
    for command in ["check", "run"] {
        let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
            .current_dir(env!("CARGO_TARGET_TMPDIR"))
            .args([command, "no_such_dir/unreadable_input_exits_1.sisu"])
            .output()
            .expect("sisu starts");
        assert_eq!(out.status.code(), Some(1), "{command}");
        assert!(
            String::from_utf8_lossy(&out.stderr)
                .starts_with("sisu: cannot read no_such_dir/unreadable_input_exits_1.sisu: "),
            "{command}"
        );
    }
}

/// GIVEN a sparse file that is one byte over 1 GiB
/// WHEN `sisu check` runs on it
/// THEN it exits 1 and stderr says the file is larger than 1 GiB
#[test]
fn check_should_exit_1_when_the_input_file_is_larger_than_1_gib() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let name = "oversized_input_is_a_tool_error.sisu";
    let file = fs::File::create(dir.join(name)).expect("creates");
    // `MAX_SOURCE` plus 1; the constant is private to the crate. The file is sparse.
    file.set_len((1 << 30) + 1).expect("sets the length");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["check", name])
        .output()
        .expect("sisu starts");
    fs::remove_file(dir.join(name)).expect("removes");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        format!("sisu: cannot read {name}: the file is larger than 1 GiB\n")
    );
}

/// GIVEN a valid program sent through a pipe to `/dev/stdin`
/// WHEN `sisu check /dev/stdin` runs
/// THEN it exits 0 and stderr is empty
#[test]
fn check_should_exit_0_when_the_source_comes_from_a_pipe() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .args(["check", "/dev/stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("sisu starts");
    // Dropping the handle closes the pipe, so sisu sees the end of the source.
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(b"fn main() {\n    print(1)\n}\n")
        .expect("writes");
    let out = child.wait_with_output().expect("sisu runs");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
}

/// Runs `sisu check` on a file named `name` that holds `bytes`; returns the exit code and
/// stderr.
fn check_bytes(name: &str, bytes: &[u8]) -> (Option<i32>, String) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join(name), bytes).expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["check", name])
        .output()
        .expect("sisu starts");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into(),
    )
}

/// GIVEN a file with two non-ASCII characters on line 1 and an invalid byte on line 2
/// WHEN `sisu check` runs on it
/// THEN it exits 1 and reports E0072 at 2:12, a column that counts characters and not bytes
#[test]
fn check_should_report_e0072_at_the_character_column_when_line_two_is_not_utf8() {
    // Line 1 holds two non-ASCII chars on four bytes: the column counts chars, not bytes.
    let name = "invalid_utf8_line_two.sisu";
    let (code, stderr) = check_bytes(name, b"// \xc3\xa4\xc3\xa4\nfn main() {\xff}\n");
    assert_eq!(code, Some(1));
    assert!(
        stderr.starts_with(&format!(
            "error[E0072]: source is not valid UTF-8\n --> {name}:2:12\n"
        )),
        "{stderr}"
    );
}

/// GIVEN a file whose first byte is invalid UTF-8
/// WHEN `sisu check` runs on it
/// THEN it exits 1 and reports E0072 at 1:1
#[test]
fn check_should_report_e0072_at_1_1_when_the_file_starts_with_an_invalid_byte() {
    let name = "invalid_utf8_start.sisu";
    let (code, stderr) = check_bytes(name, b"\xff\nfn main() {}\n");
    assert_eq!(code, Some(1));
    assert!(
        stderr.starts_with(&format!(
            "error[E0072]: source is not valid UTF-8\n --> {name}:1:1\n"
        )),
        "{stderr}"
    );
}

/// GIVEN a file that ends in the first two bytes of a three-byte character
/// WHEN `sisu check` runs on it
/// THEN it exits 1 and reports E0072 at 2:1
#[test]
fn check_should_report_e0072_at_the_start_of_a_truncated_character_at_the_end_of_the_file() {
    // `\xe2\x82` starts the three-byte `€` and stops.
    let name = "invalid_utf8_truncated.sisu";
    let (code, stderr) = check_bytes(name, b"fn main() {}\n\xe2\x82");
    assert_eq!(code, Some(1));
    assert!(
        stderr.starts_with(&format!(
            "error[E0072]: source is not valid UTF-8\n --> {name}:2:1\n"
        )),
        "{stderr}"
    );
}

/// GIVEN files with CRLF line breaks and an invalid byte after two breaks, and right after one
/// break
/// WHEN `sisu check` runs on them
/// THEN it reports E0072 at 3:3 for the first file and at 2:1 for the second
#[test]
fn check_should_count_each_crlf_as_one_line_when_it_reports_invalid_utf8() {
    let (name, name_start) = ("invalid_utf8_crlf.sisu", "invalid_utf8_crlf_start.sisu");
    let (code, stderr) = check_bytes(name, b"a\r\nb\r\nxy\xff\r\n");
    assert_eq!(code, Some(1));
    assert!(
        stderr.starts_with(&format!(
            "error[E0072]: source is not valid UTF-8\n --> {name}:3:3\n"
        )),
        "{stderr}"
    );
    // The bad byte is the first of its line, right after the break.
    let (_, stderr) = check_bytes(name_start, b"a\r\n\xff\r\n");
    assert!(
        stderr.starts_with(&format!(
            "error[E0072]: source is not valid UTF-8\n --> {name_start}:2:1\n"
        )),
        "{stderr}"
    );
}

/// GIVEN a file with the bytes of the surrogate U+D800 after `xy` on line 2
/// WHEN `sisu check` runs on it
/// THEN it exits 1 and reports E0072 at 2:3
#[test]
fn check_should_report_e0072_at_the_first_byte_of_an_encoded_surrogate() {
    // `\xed\xa0\x80` would be U+D800, which UTF-8 does not allow.
    let name = "invalid_utf8_surrogate.sisu";
    let (code, stderr) = check_bytes(name, b"a\nxy\xed\xa0\x80\n");
    assert_eq!(code, Some(1));
    assert!(
        stderr.starts_with(&format!(
            "error[E0072]: source is not valid UTF-8\n --> {name}:2:3\n"
        )),
        "{stderr}"
    );
}

/// GIVEN the program `loops.sisu`, which declares a `let mut` inside a loop body
/// WHEN it is built with `--emit ir`
/// THEN the module before `mem2reg` has an `alloca` and the module after it has none
#[test]
fn emit_ir_should_show_allocas_before_mem2reg_and_none_after() {
    // `loops.sisu` declares `let mut sq` inside the loop body: its `alloca` must still sit in the
    // entry block, or `mem2reg` leaves it in place.
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/run"))
        .args(["build", "--emit", "ir", "loops.sisu"])
        .output()
        .expect("sisu starts");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let (before, after) = ir_sections(&stdout);
    assert!(before.contains("alloca"), "{before}");
    assert!(!after.contains("alloca"), "{after}");
}

/// Splits `--emit ir` output into the modules before and after `mem2reg`, and checks that
/// each defines `main`.
fn ir_sections(stdout: &str) -> (&str, &str) {
    let before = stdout
        .strip_prefix("; before mem2reg\n")
        .unwrap_or_else(|| panic!("no `; before mem2reg` header in\n{stdout}"));
    let (before, after) = before
        .split_once("\n; after mem2reg\n")
        .unwrap_or_else(|| panic!("no `; after mem2reg` line in\n{stdout}"));
    for section in [before, after] {
        assert!(
            section.contains("define internal void @sisu.main()"),
            "{section}"
        );
    }
    (before, after)
}

/// GIVEN a program that assigns to an immutable variable
/// WHEN it is built with `--emit ir`
/// THEN it exits 1, stderr starts with the error E0001, and stdout is empty
#[test]
fn emit_ir_should_exit_1_and_print_no_ir_when_the_program_has_an_error() {
    let out = run_on(
        "emit_ir_error_prints_no_ir",
        "fn main() {\n    let n = 0\n    n = 1\n}\n",
        &["build", "--emit", "ir"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("error[E0001]: cannot assign to `n`"));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
}

/// GIVEN a program with a `let mut` variable that is never reassigned
/// WHEN it is built with `--emit ir`
/// THEN it exits 0, stderr starts with the warning W0002, and stdout holds both IR sections
#[test]
fn emit_ir_should_print_the_ir_and_the_warning_when_the_program_has_only_a_warning() {
    let out = run_on(
        "emit_ir_warning_still_prints_ir",
        "fn main() {\n    let mut count = 0\n    print(count)\n}\n",
        &["build", "--emit", "ir"],
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .starts_with("warning[W0002]: `count` is never reassigned")
    );
    ir_sections(&String::from_utf8_lossy(&out.stdout));
}

/// GIVEN a program with a redundant semicolon
/// WHEN it is built with `--emit tokens` and with `--emit ast`
/// THEN each run exits 0, stderr starts with the warning W0007, and stdout has the stage's output
///
/// A warning does not stop `--emit tokens` or `--emit ast`, as it does not stop `check`.
#[test]
fn emit_tokens_and_ast_should_print_and_exit_0_when_the_program_has_a_warning() {
    for (stage, first) in [("tokens", "1:1 Fn fn\n"), ("ast", "(fn main ")] {
        let out = run_on(
            "emit_with_a_redundant_semicolon_still_prints",
            "fn main() {\n    print(1);\n}\n",
            &["build", "--emit", stage],
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{stage}: {stderr}");
        assert!(stderr.starts_with("warning[W0007]"), "{stage}: {stderr}");
        assert!(
            String::from_utf8_lossy(&out.stdout).starts_with(first),
            "{stage}"
        );
    }
}

/// GIVEN a program with a `while` loop over a mutable counter
/// WHEN it is built with `--emit tir`
/// THEN stdout shows the loop as `loop`, with an `if` whose else branch is a `break`
#[test]
fn emit_tir_should_print_a_while_loop_as_a_loop_with_a_break() {
    let out = run_on(
        "emit_tir",
        "fn main() {\n    let mut i = 0\n    while i < 2 {\n        i = i + 1\n    }\n}\n",
        &["build", "--emit", "tir"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "(fn main () unit (block (let mut i#0 0) (loop (block (if (< i#0 2) (block (= i#0 (+ i#0 1))) (block (break)))))))\n"
    );
}

/// GIVEN a function that returns its array parameter
/// WHEN it is built with `--emit mir`
/// THEN stdout has the line `= move xs#0`
#[test]
fn emit_mir_should_show_a_move_when_a_function_returns_its_parameter() {
    let out = run_on(
        "emit_mir_shows_moves",
        "fn _f(xs: [i64]) -> [i64] { xs }\nfn main() {}\n",
        &["build", "--emit", "mir"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("= move xs#0"));
}

/// GIVEN a function that unwraps its `string??` parameter with two `if let`s
/// WHEN it is built with `--emit mir`
/// THEN `f` has two `as some` lines, one per layer, each a borrow, and no copy or retain
///
/// Each `if let` takes one layer, and a borrowed parameter's layers are borrowed, not retained.
#[test]
fn emit_mir_should_borrow_each_layer_of_a_nested_optional_parameter() {
    let out = run_on(
        "emit_mir_nested_optional",
        "fn f(x: string??) -> i64 {\n    if let y = x {\n        if let z = y {\n            return z.len()\n        }\n    }\n    0\n}\nfn main() {\n    print(f(None))\n}\n",
        &["build", "--emit", "mir"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let f = stdout
        .split("\nfn ")
        .find(|f| f.starts_with("fn f(") || f.starts_with("f("))
        .unwrap_or_else(|| panic!("`f` should be printed: {stdout}"));
    let layers: Vec<&str> = f.lines().filter(|l| l.contains(" as some)")).collect();
    assert_eq!(layers.len(), 2, "{f}");
    assert!(layers.iter().all(|l| l.contains("= borrow (")), "{f}");
    assert!(!f.contains("copy") && !f.contains("retain"), "{f}");
}

/// GIVEN a program with an enum, a `match` and a `main`
/// WHEN it is built with `--emit mir`
/// THEN stdout has the function `main` and the block `bb0:`
///
/// `--emit mir` prints every function's blocks.
#[test]
fn emit_mir_should_print_the_blocks_of_each_function() {
    let out = run_on(
        "emit_mir",
        "enum E {\n    A(string)\n    B\n}\n\nfn name(e: E) -> string {\n    match e {\n        A(s) => s\n        B => \"b\"\n    }\n}\n\nfn main() {\n    print(name(E.A(\"a\")))\n    print(name(E.B))\n}\n",
        &["build", "--emit", "mir"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("fn main(") && stdout.contains("bb0:"),
        "{stdout}"
    );
}

/// GIVEN the stage name `bogus`
/// WHEN `sisu build --emit bogus` runs
/// THEN it exits 2 and the error lists the stages `tokens, ast, tir, mir, ir, ir-raw`
///
/// A stage `--emit` does not know is a usage error, and clap's error lists `mir` among the stages.
#[test]
fn emit_should_list_mir_among_the_possible_stages_when_the_stage_is_unknown() {
    let stderr = assert_usage_errors(&[&["build", "--emit", "bogus", "f.sisu"]]);
    assert!(
        stderr.contains("[possible values: tokens, ast, tir, mir, ir, ir-raw]"),
        "{stderr}"
    );
}

/// GIVEN a program that assigns to an immutable variable
/// WHEN it is built with `--emit tir`, with `--emit mir`, and with `--format json --emit tir`
/// THEN each run exits 1, stderr starts with the error E0001 in the format asked for, and stdout is
/// empty
#[test]
fn emit_should_print_nothing_on_stdout_when_the_program_has_an_error() {
    let flags: [(&[&str], &str); 3] = [
        (&["build", "--emit", "tir"], "error[E0001]: "),
        (&["build", "--emit", "mir"], "error[E0001]: "),
        (
            &["build", "--format", "json", "--emit", "tir"],
            r#"{"version":1,"kind":"diagnostic","code":"E0001","#,
        ),
    ];
    for (flags, first) in flags {
        let out = run_on(
            "emit_with_an_error_prints_nothing_on_stdout",
            "fn main() {\n    let n = 0\n    n = 1\n}\n",
            flags,
        );
        assert_eq!(out.status.code(), Some(1), "{flags:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.starts_with(first), "{flags:?}: {stderr}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "", "{flags:?}");
    }
}

/// GIVEN a valid program
/// WHEN it is built with each `--emit` stage, in text format and in JSON format
/// THEN stdout is the same in both formats, and stderr holds only the JSON summary in JSON format
///
/// On a valid file, `--format json` leaves stdout as text has it, and stderr holds only the
/// summary.
#[test]
fn json_format_should_not_change_the_emitted_stage_on_stdout() {
    for stage in ["tokens", "ast", "tir", "mir", "ir", "ir-raw"] {
        let text = run_on(
            "json_emit_keeps_stdout",
            SQUARE,
            &["build", "--emit", stage],
        );
        let json = run_on(
            "json_emit_keeps_stdout",
            SQUARE,
            &["build", "--format", "json", "--emit", stage],
        );
        for out in [&text, &json] {
            assert_eq!(
                out.status.code(),
                Some(0),
                "{stage}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        assert!(!text.stdout.is_empty(), "{stage}");
        assert_eq!(json.stdout, text.stdout, "{stage}");
        assert_eq!(String::from_utf8_lossy(&text.stderr), "", "{stage}");
        assert_eq!(
            String::from_utf8_lossy(&json.stderr),
            format!("{CLEAN_SUMMARY}\n"),
            "{stage}"
        );
    }
}

const FIB: &str = "fn fib(n: i64) -> i64 {\n    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }\n}\n\nfn main() {\n    print(fib(30))\n}\n";

/// GIVEN `-O2` with `explain`, `check`, a stage other than `ir`, no file or a repeat
/// WHEN `sisu` runs
/// THEN it exits 2 with a usage error
#[test]
fn o2_should_exit_2_when_the_command_or_the_stage_does_not_allow_it() {
    let cases: [&[&str]; 13] = [
        &["explain", "-O2", "E0001"],
        &["build", "-O2", "--emit", "mir", "f.sisu"],
        &["explain", "E0001", "-O2"],
        &["check", "-O2", "f.sisu"],
        &["build", "-O2", "--emit", "tokens", "f.sisu"],
        &["build", "-O2", "--emit", "ast", "f.sisu"],
        &["build", "-O2", "--emit", "tir", "f.sisu"],
        &["build", "-O2", "--emit", "ir-raw", "f.sisu"],
        &["check", "f.sisu", "-O2"],
        &["build", "-O2", "-O2", "f.sisu", "-o", "out"],
        &["build", "--emit", "ir", "-O2"],
        &["check", "-O2"],
        &["build", "--emit", "ir-raw", "-O2"],
    ];
    assert_usage_errors(&cases);
}

/// GIVEN `-O3` with `-O2`, `check` or `--emit ir-raw`, or `--linker` with `check`, `--emit`, a
/// repeat
/// or an unknown linker
/// WHEN `sisu` runs
/// THEN it exits 2 with a usage error
/// AND the error for `--linker` with `--emit ir` says the flag works only when building an
/// executable
///
/// `-O3` takes `-O2`'s place, once, and `--linker` goes with a build only, once.
#[test]
fn o3_and_linker_should_exit_2_when_the_command_does_not_allow_them() {
    let cases: [&[&str]; 9] = [
        &["build", "-O2", "-O3", "f.sisu", "-o", "out"],
        &["build", "-O3", "-O2", "f.sisu", "-o", "out"],
        &["check", "-O3", "f.sisu"],
        &["build", "-O3", "--emit", "ir-raw", "f.sisu"],
        &["build", "--linker", "lld", "--emit", "ir", "f.sisu"],
        &["check", "--linker", "lld", "f.sisu"],
        &[
            "build", "--linker", "lld", "--linker", "lld", "f.sisu", "-o", "out",
        ],
        &["build", "--linker", "ld.lld", "f.sisu", "-o", "out"],
        &["build", "--linker"],
    ];
    assert_usage_errors(&cases);
    let stderr = assert_usage_errors(&[&["build", "--linker", "lld", "--emit", "ir", "f.sisu"]]);
    assert!(
        stderr.starts_with("error: `--linker` works only when building an executable\n"),
        "{stderr}"
    );
}

/// GIVEN `--allocator` with an unknown allocator, with `check`, with a repeat or with `--emit ir`
/// WHEN `sisu` runs
/// THEN it exits 2 with a usage error
/// AND the unknown-allocator error names `mimalloc` and `system`, and the `--emit ir` error says
/// the
/// flag works only when building an executable
///
/// `--allocator` names one of the two runtimes, and goes with a build only, once: neither
/// runtime changes the emitted IR, so even the default, `mimalloc`, is refused with `--emit`.
#[test]
fn allocator_should_exit_2_unless_the_command_builds_an_executable() {
    let stderr = assert_usage_errors(&[&["build", "--allocator", "glibc", "f.sisu", "-o", "out"]]);
    assert!(
        stderr.contains("mimalloc") && stderr.contains("system"),
        "{stderr}"
    );
    assert_usage_errors(&[
        &["check", "--allocator", "system", "f.sisu"],
        &[
            "build",
            "--allocator",
            "system",
            "--allocator",
            "system",
            "f.sisu",
            "-o",
            "out",
        ],
    ]);
    for allocator in ["mimalloc", "system"] {
        let stderr =
            assert_usage_errors(&[&["build", "--allocator", allocator, "--emit", "ir", "f.sisu"]]);
        assert!(
            stderr.starts_with("error: `--allocator` works only when building an executable\n"),
            "{allocator}: {stderr}"
        );
    }
}

/// GIVEN a program that computes `fib(30)`
/// WHEN it is built with `--emit ir-raw`
/// THEN LLVM parses the output into a module that verifies, and the output has no `; before` header
#[test]
fn emit_ir_raw_should_print_ir_that_the_llvm_parser_reads_and_verifies() {
    let out = run_on(
        "emit_ir_raw_parses_back",
        FIB,
        &["build", "--emit", "ir-raw"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let context = inkwell::context::Context::create();
    let module = context
        .create_module_from_ir(
            inkwell::memory_buffer::MemoryBuffer::create_from_memory_range_copy(&out.stdout, "raw"),
        )
        .unwrap_or_else(|e| panic!("{e}\n{}", String::from_utf8_lossy(&out.stdout)));
    module.verify().expect("the module verifies");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("; before"));
}

/// GIVEN the program `loops.sisu`
/// WHEN it is built with `--emit ir-raw` and with `--emit ir`
/// THEN the raw output equals the section of the `ir` output before `mem2reg`
#[test]
fn emit_ir_raw_should_equal_the_before_section_of_emit_ir() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/run");
    let emit = |stage| {
        let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
            .current_dir(dir)
            .args(["build", "--emit", stage, "loops.sisu"])
            .output()
            .expect("sisu starts");
        assert_eq!(out.status.code(), Some(0));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let (ir, raw) = (emit("ir"), emit("ir-raw"));
    let (before, _) = ir_sections(&ir);
    assert_eq!(raw.trim_end(), before.trim_end());
}

const SQUARE: &str =
    "fn sq(n: i64) -> i64 {\n    n * n\n}\n\nfn main() {\n    print(sq(3) + 4)\n}\n";

/// GIVEN a program that calls a small function and prints the sum of the result and 4
/// WHEN it is built with `-O2 --emit ir`
/// THEN the output has `default<O2>` sections, the call is in the before section only, and the
/// after
/// section prints the constant 13 and holds the target's data layout and triple
#[test]
fn emit_ir_at_o2_should_inline_and_fold_a_call_in_the_after_section() {
    let out = run_on("emit_ir_o2", SQUARE, &["build", "-O2", "--emit", "ir"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("; before default<O2>\n"), "{stdout}");
    let (before, after) = stdout
        .split_once("\n; after default<O2>\n")
        .unwrap_or_else(|| panic!("{stdout}"));
    // Only the O2 pipeline inlines and folds the call; mem2reg alone keeps it.
    assert!(before.contains("call i64 @sisu.sq(i64 3)"), "{before}");
    assert!(!after.contains("call i64 @sisu.sq"), "{after}");
    assert!(after.contains("@sisu_print_int(i64 13)"), "{after}");
    // The passes see the target's triple and data layout, as the object file does.
    for wanted in ["\ntarget datalayout = \"", "\ntarget triple = \""] {
        assert!(after.contains(wanted), "missing {wanted:?} in\n{after}");
    }
}

/// A loop that branches on a flag it never changes, which comes from input so that it does not
/// fold away.
const UNSWITCH: &str = "fn main() {\n    let loud = (read_line() ?? \"\") == \"loud\"\n\
                        let mut total = 0\n    for i in 0..1000 {\n\
                        if loud { print(i) } else { total += i }\n    }\n    print(total)\n}\n";

/// The loop latches in `ir`: branches to a block of the same function at or above the branch.
fn latches(ir: &str) -> usize {
    let mut blocks = HashSet::new();
    let mut latches = 0;
    for line in ir.lines() {
        if line.starts_with("define ") {
            blocks.clear();
        } else if let Some((label, _)) = line.split_once(':')
            && !line.starts_with([' ', ';'])
            && !label.contains(' ')
        {
            blocks.insert(format!("%{label}"));
        } else if line.trim_start().starts_with("br ") {
            latches += line
                .split("label ")
                .skip(1)
                .filter(|target| blocks.contains(target.split(',').next().unwrap_or_default()))
                .count();
        }
    }
    latches
}

/// GIVEN a loop that branches on a flag it never changes and does work on both sides
/// WHEN it is built with `-O2 --emit ir` and with `-O3 --emit ir`
/// THEN each output has the sections of its own pipeline, and the `-O3` output has more loops than
/// the
/// `-O2` output
///
/// Only `default<O3>` unswitches a loop whose branch does work on both sides: it splits the
/// loop into one per side, so the program has more loops than at `default<O2>`.
#[test]
fn emit_ir_at_o3_should_have_more_loops_than_at_o2_when_a_loop_can_unswitch() {
    let loops = |level: &str| {
        let out = run_on("emit_ir_o3", UNSWITCH, &["build", level, "--emit", "ir"]);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert_eq!(out.status.code(), Some(0), "{stdout}");
        assert!(
            stdout.starts_with(&format!("; before default<{}>\n", &level[1..])),
            "{stdout}"
        );
        let (_, after) = stdout
            .split_once(&format!("\n; after default<{}>\n", &level[1..]))
            .unwrap_or_else(|| panic!("{stdout}"));
        latches(after)
    };
    let (o2, o3) = (loops("-O2"), loops("-O3"));
    assert!(o2 > 0, "no loop at -O2");
    assert!(o3 > o2, "-O3 has {o3} loops, -O2 {o2}");
}

/// GIVEN a nested loop that reads an array and may call `print`
/// WHEN it is built with `-O2 --emit ir`
/// THEN the before section calls `@sisu_panic_bounds` and the after section does not
///
/// A `print` that may never run does not make the loop reload the array's length: the
/// runtime's print functions write only memory the module cannot reach, so `-O2` proves
/// every `xs[i]` in range from the loop bound and drops both bounds checks.
#[test]
fn print_in_a_loop_should_not_keep_a_bounds_check_at_o2() {
    let source = "fn main() {\n    let xs = [1; 10_000]\n    let mut total = 0\n\
                  for _ in 0..1000 {\n        for i in 0..10_000 {\n\
                  if xs[i] == 0 { print(i) }\n            total += xs[i]\n        }\n    }\n\
                  print(total)\n}\n";
    let out = run_on("print_in_a_loop", source, &["build", "-O2", "--emit", "ir"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let (before, after) = stdout
        .split_once("\n; after default<O2>\n")
        .unwrap_or_else(|| panic!("{stdout}"));
    assert!(before.contains("call void @sisu_panic_bounds("), "{before}");
    assert!(!after.contains("call void @sisu_panic_bounds("), "{after}");
}

/// GIVEN a `for i in 0..=n` loop over constant bounds that sums its counter
/// WHEN it is built with `-O2 --emit ir`
/// THEN the after section prints the constant 55 and has no `with.overflow` call
///
/// A `for ..=` loop steps its counter only while it is below the end, so the step needs no
/// overflow check, and `-O2` folds a loop over constant bounds as it does over `..`.
#[test]
fn inclusive_for_loop_should_fold_to_a_constant_without_overflow_checks_at_o2() {
    let source = "fn main() {\n    let n = 10\n    let mut total = 0\n\
                  for i in 0..=n {\n        total += i\n    }\n    print(total)\n}\n";
    let out = run_on(
        "inclusive_for_folds",
        source,
        &["build", "-O2", "--emit", "ir"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let (_, after) = stdout
        .split_once("\n; after default<O2>\n")
        .unwrap_or_else(|| panic!("{stdout}"));
    assert!(after.contains("@sisu_print_int(i64 55)"), "{after}");
    assert!(!after.contains("with.overflow"), "{after}");
}

/// The JSON summary line of a run with no diagnostics.
const CLEAN_SUMMARY: &str =
    r#"{"version":1,"kind":"summary","errors":0,"warnings":0,"explanations":{}}"#;

/// GIVEN a path whose directory does not exist
/// WHEN `sisu check --format json` runs on it
/// THEN it exits 1 and stderr is a `tool_error` record followed by the clean summary
#[test]
fn check_with_json_format_should_report_an_unreadable_input_as_a_tool_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(env!("CARGO_TARGET_TMPDIR"))
        .args(["check", "--format", "json", "no_such_dir/missing.sisu"])
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr)
            .lines()
            .collect::<Vec<_>>(),
        [
            r#"{"version":1,"kind":"tool_error","message":"cannot read no_such_dir/missing.sisu: No such file or directory (os error 2)"}"#,
            CLEAN_SUMMARY
        ]
    );
}

/// GIVEN `--format` with no value, with the value `yaml`, or with `explain`
/// WHEN `sisu` runs
/// THEN it exits 2 with a usage error in text, also when `--format json` is in use
///
/// `--format` takes `text` or `json`, and never goes with `explain`. A usage error is clap's
/// text, also with `--format json`.
#[test]
fn format_flag_should_exit_2_with_clap_text_when_it_is_misused() {
    assert_usage_errors(&[
        &["check", "--format", "json"],
        &["check", "--format", "yaml", "a.sisu"],
        &["explain", "--format", "text", "E0001"],
        &["check", "--format"],
        &["build", "--emit", "tir", "--format"],
        &["build", "-O2", "--emit", "ir", "--format"],
        &["explain", "--format"],
    ]);
}

/// GIVEN a `PATH` with only `cc`, which cannot find the linker `mold`
/// WHEN `sisu build --format json --linker mold` runs
/// THEN it exits 1 and every stderr line is JSON, the last line is the summary, and one
/// `tool_error`
/// holds the exit status and the output of `cc`
#[test]
fn build_with_json_format_should_report_a_link_failure_as_one_tool_error() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("json_link_failure.sisu"), "fn main() {}\n").expect("writes");
    // With only `cc` on `PATH`, `cc` fails to find `ld.mold`, for any user.
    let only_cc = scratch_path::only_cc("json_link_failure_path");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .env("PATH", &only_cc)
        .env_remove("COMPILER_PATH")
        .args([
            "build",
            "--format",
            "json",
            "--linker",
            "mold",
            "json_link_failure.sisu",
            "-o",
            "json_link_failure_out",
        ])
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr.lines().collect();
    assert!(
        lines
            .iter()
            .all(|l| l.starts_with(r#"{"version":1,"kind":""#)),
        "{stderr}"
    );
    assert_eq!(lines.last(), Some(&CLEAN_SUMMARY), "{stderr}");
    let messages: Vec<String> = lines
        .iter()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("each line is JSON"))
        .filter(|v| v["kind"] == "tool_error")
        .map(|v| v["message"].as_str().expect("a string message").to_string())
        .collect();
    let [message] = messages.as_slice() else {
        panic!("one tool error in {stderr}");
    };
    // cc's own stderr follows the status.
    let (status, detail) = message
        .split_once('\n')
        .expect("cc's output after a newline");
    assert!(status.ends_with("cc failed: exit status: 1"), "{message}");
    assert!(!detail.is_empty(), "{message}");
    assert_eq!(detail, detail.trim(), "{message}");
}

/// GIVEN an output path that is the input file
/// WHEN `sisu build --format json` runs
/// THEN it exits 1, stderr is a `tool_error` record and the summary, and the input file is
/// unchanged
#[test]
fn build_with_json_format_should_refuse_an_output_that_would_overwrite_the_input() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("json_overwrite.sisu"), "fn main() {}\n").expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args([
            "build",
            "--format",
            "json",
            "json_overwrite.sisu",
            "-o",
            "json_overwrite.sisu",
        ])
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr)
            .lines()
            .collect::<Vec<_>>(),
        [
            r#"{"version":1,"kind":"tool_error","message":"output json_overwrite.sisu would overwrite the input"}"#,
            CLEAN_SUMMARY
        ]
    );
    assert_eq!(
        fs::read_to_string(dir.join("json_overwrite.sisu")).expect("reads"),
        "fn main() {}\n"
    );
}

/// GIVEN a program with a `let mut` variable that is never reassigned
/// WHEN it is built to an executable
/// THEN it exits 0, stderr ends with the summary for W0002, and the executable prints 3
#[test]
fn build_should_exit_0_and_print_a_warning_summary_when_the_program_has_only_a_warning() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(
        dir.join("warnings_only_build.sisu"),
        "fn main() {\n    let mut count = 3\n    print(count)\n}\n",
    )
    .expect("writes");
    let exe = dir.join("warnings_only_build");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["build", "warnings_only_build.sisu", "-o"])
        .arg(&exe)
        .output()
        .expect("sisu starts");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.ends_with(
            "\n\nsisu: 1 warning\nsisu: for more on a code, run `sisu explain <code>`: W0002\n"
        ),
        "{stderr}"
    );
    let run = Command::new(&exe).output().expect("the program starts");
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&run.stdout), "3\n");
}

/// GIVEN a file with an error
/// WHEN `sisu check` runs with and without `--format text`
/// THEN both runs exit 1, print the same stderr, and it starts with the text error E0001
#[test]
fn check_should_print_text_diagnostics_when_the_format_flag_is_absent() {
    let source = "fn main() {\n    let n = 0\n    n = 1\n}\n";
    let plain = run_on("format_text_is_the_default", source, &["check"]);
    let text = run_on(
        "format_text_is_the_default",
        source,
        &["check", "--format", "text"],
    );
    assert_eq!(text.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&text.stderr).starts_with("error[E0001]: "),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    assert_eq!(text.stderr, plain.stderr);
}

/// GIVEN enums declared before the enums they hold, an enum with no fields and an enum with 257
/// variants
/// WHEN they are built with `--emit ir`
/// THEN each enum type has the smallest tag that holds its variants, and storage for its largest
/// payload
/// AND the enum with no fields has no storage
#[test]
fn emit_ir_should_size_each_enum_by_its_variant_count_and_largest_payload() {
    use std::fmt::Write as _;
    // `Outer` is declared before the `Pair?` it holds and `Holder` before the `Trio` it holds,
    // so each layout needs a later one's first. `Wide` has 257 variants, one more than an `i8`
    // tag holds.
    let wide = (0..257).fold(String::new(), |mut wide, v| {
        writeln!(wide, "    V{v}").expect("writing to a `String` should not fail");
        wide
    });
    // `main` builds each variant that the comparisons do not, so that none of them is unused.
    let builds = (1..256).fold(String::new(), |mut builds, v| {
        writeln!(builds, "    let _ = Wide.V{v}").expect("writing to a `String` should not fail");
        builds
    });
    let source = format!(
        "enum Outer {{\n    Maybe(Pair?)\n}}\n\n\
         enum Pair {{\n    Two(i64, i64)\n}}\n\n\
         enum Holder {{\n    Wrap(Trio)\n}}\n\n\
         enum Trio {{\n    Three(i64, i64, i64)\n}}\n\n\
         enum Light {{\n    Red\n    Green\n}}\n\n\
         enum Wide {{\n{wide}}}\n\n\
         fn main() {{\n    \
         print(Outer.Maybe(Pair.Two(1, 2)) == Outer.Maybe(None))\n    \
         print(Holder.Wrap(Trio.Three(1, 2, 3)) == Holder.Wrap(Trio.Three(1, 2, 4)))\n    \
         print(Light.Red == Light.Green)\n    \
         print(Wide.V256 == Wide.V0)\n\
         {builds}}}\n"
    );
    let out = run_on(
        "enum_layouts_follow_the_target",
        &source,
        &["build", "--emit", "ir"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = String::from_utf8_lossy(&out.stdout);
    // The smallest tag that holds the variant count; storage sized and aligned for the largest
    // payload, a `Pair?` at the 24 bytes of `%sisu.Pair` (its `None` is a spare tag) and a
    // `%sisu.Trio` at 32 bytes; no storage when no variant has a field.
    for wanted in [
        "%sisu.Pair = type { i8, [2 x i64] }",
        "%sisu.Outer = type { i8, [3 x i64] }",
        "%sisu.Trio = type { i8, [3 x i64] }",
        "%sisu.Holder = type { i8, [4 x i64] }",
        "%sisu.Light = type { i8 }",
        "%sisu.Wide = type { i16 }",
    ] {
        assert!(ir.contains(wanted), "missing {wanted:?} in\n{ir}");
    }
}

/// GIVEN a copy of `sisu` with no runtime library beside it, and a temporary directory that is
/// absent
/// WHEN it runs `build` and `run`, with the default allocator and with `--allocator system`
/// THEN each run exits 1 and stderr says the runtime library is not found, with the archive of the
/// allocator
///
/// A `sisu` with no runtime library beside it says so before it compiles: with no
/// temporary directory, a compile would fail at the object file first.
#[test]
fn build_and_run_should_report_a_missing_runtime_library_before_they_compile() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("missing_runtime");
    drop(fs::remove_dir_all(&dir));
    fs::create_dir_all(&dir).expect("creates");
    let sisu = dir.join("sisu");
    fs::hard_link(env!("CARGO_BIN_EXE_sisu"), &sisu).expect("links");
    fs::write(dir.join("one.sisu"), "fn main() {\n    print(1)\n}\n").expect("writes");
    for args in [
        &["build", "one.sisu", "-o", "one"][..],
        &["run", "one.sisu"],
        &["build", "--allocator", "system", "one.sisu", "-o", "one"],
        &["run", "--allocator", "system", "one.sisu"],
    ] {
        let out = Command::new(&sisu)
            .current_dir(&dir)
            .env("TMPDIR", dir.join("absent"))
            .args(args)
            .output()
            .expect("sisu starts");
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.starts_with("sisu: runtime library not found at "),
            "{args:?}: {stderr}"
        );
        let archive = if args.contains(&"system") {
            "/libsisu_runtime_system.a;"
        } else {
            "/libsisu_runtime_mimalloc.a;"
        };
        assert!(stderr.contains(archive), "{args:?}: {stderr}");
    }
}

/// GIVEN output paths in a missing directory, in a regular file, under a regular file, and a
/// directory
/// WHEN `sisu build -o` runs with each path
/// THEN each run exits 1 and stderr is the message for that path, so the compile does not start
///
/// An output whose directory does not exist, or that is a directory, is reported before
/// the compile.
#[test]
fn build_should_report_a_bad_output_path_before_it_compiles() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("bad_output_path");
    drop(fs::remove_dir_all(&dir));
    fs::create_dir_all(dir.join("outdir")).expect("creates");
    fs::write(dir.join("regular"), "").expect("writes");
    fs::write(dir.join("one.sisu"), "fn main() {\n    print(1)\n}\n").expect("writes");
    for (output, expected) in [
        (
            "missing/one",
            "sisu: output directory missing does not exist\n",
        ),
        ("outdir", "sisu: output outdir is a directory\n"),
        (
            "regular/one",
            "sisu: output directory regular is not a directory\n",
        ),
        (
            "regular/sub/one",
            "sisu: cannot access output directory regular/sub: Not a directory (os error 20)\n",
        ),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
            .current_dir(&dir)
            .env("TMPDIR", dir.join("absent"))
            .args(["build", "one.sisu", "-o", output])
            .output()
            .expect("sisu starts");
        assert_eq!(out.status.code(), Some(1), "{output}");
        assert_eq!(String::from_utf8_lossy(&out.stderr), expected);
    }
}

/// GIVEN `--rc-checks`, `-O2`, `-O3` and `--linker`, placed before or after the file
/// WHEN `sisu build` runs
/// THEN each build exits 0 and its executable prints 1
/// AND `--emit ir -O2` after the file gives the `default<O2>` sections
///
/// `build`'s flags go before or after the file, in any order. The `--linker` rows run on Linux
/// only: Apple's clang has no `bfd`, and for `lld` it looks for `ld64.lld`, which `.llvm/23` lacks.
#[test]
fn build_should_accept_its_flags_in_any_order_before_or_after_the_file() {
    let dir = scratch_path::empty_dir("flags_in_any_order");
    fs::write(dir.join("f.sisu"), "fn main() {\n    print(1)\n}\n").expect("writes");
    // `ld.lld` is in LLVM's `bin`.
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(Path::new(env!("LLVM_SYS_231_PREFIX")).join("bin"))
            .chain(std::env::split_paths(&path)),
    )
    .expect("PATH joins");
    let sisu = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
            .current_dir(&dir)
            .env("PATH", &path)
            .args(args)
            .output()
            .expect("sisu starts");
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    };
    for args in [
        &[
            "build",
            "--rc-checks",
            "-O2",
            "--linker",
            "bfd",
            "f.sisu",
            "-o",
            "e",
        ][..],
        &["build", "f.sisu", "-o", "e", "-O2", "--rc-checks"],
        &["build", "--rc-checks", "-O3", "f.sisu", "-o", "e"],
        &[
            "build",
            "--linker",
            "lld",
            "--rc-checks",
            "f.sisu",
            "-o",
            "e",
        ],
        &["build", "f.sisu", "-o", "e", "--linker", "lld"],
    ]
    .into_iter()
    .filter(|args| cfg!(target_os = "linux") || !args.contains(&"--linker"))
    {
        // It is absent before the first build, so a failure here is expected and harmless.
        drop(fs::remove_file(dir.join("e")));
        sisu(args);
        let run = Command::new(dir.join("e"))
            .output()
            .expect("the program starts");
        assert_eq!(String::from_utf8_lossy(&run.stdout), "1\n", "{args:?}");
    }
    let out = sisu(&["build", "--emit", "ir", "-O2", "f.sisu"]);
    assert!(
        String::from_utf8_lossy(&out.stdout).starts_with("; before default<O2>\n"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// GIVEN a source file in the current directory, and one in the parent directory
/// WHEN `sisu build` runs without `-o`, and with `-o`
/// THEN the executable is named after the file without its extension, in the current directory, and
/// it
/// prints the expected output
///
/// Without `-o`, `build` names the executable after the file, without its extension, in the
/// current directory, wherever the file is.
#[test]
fn build_should_name_the_executable_after_the_file_stem_in_the_current_directory() {
    let dir = scratch_path::empty_dir("build_writes_the_stem");
    let source = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/run"));
    fs::copy(source.join("loops.sisu"), dir.join("loops.sisu")).expect("copies");
    fs::create_dir(dir.join("sub")).expect("creates");
    let expected = fs::read_to_string(source.join("loops.out")).expect("reads");
    for (cwd, args, exe) in [
        (&dir, &["build", "loops.sisu"][..], dir.join("loops")),
        (
            &dir,
            &["build", "loops.sisu", "-o", "sub/x"],
            dir.join("sub/x"),
        ),
        (
            &dir.join("sub"),
            &["build", "../loops.sisu"],
            dir.join("sub/loops"),
        ),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
            .current_dir(cwd)
            .args(args)
            .output()
            .expect("sisu starts");
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let run = Command::new(&exe).output().expect("the program starts");
        assert_eq!(String::from_utf8_lossy(&run.stdout), expected, "{args:?}");
    }
}

/// GIVEN a file with an error
/// WHEN `sisu check --format json` runs
/// THEN it exits 1 and stderr is two JSON lines: the E0001 diagnostic and the summary
///
/// `--format` follows the subcommand.
#[test]
fn check_should_print_json_records_when_format_follows_the_subcommand() {
    let out = run_on(
        "format_after_the_subcommand",
        "fn main() {\n    let n = 0\n    n = 1\n}\n",
        &["check", "--format", "json"],
    );
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<serde_json::Value> = stderr
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
        .collect();
    assert_eq!(lines.len(), 2, "{stderr}");
    assert_eq!(lines[0]["code"], "E0001", "{stderr}");
    assert_eq!(lines[1]["kind"], "summary", "{stderr}");
}

/// GIVEN `--emit ast` with `-o x`
/// WHEN `sisu build` runs
/// THEN it exits 2 with a usage error
///
/// `--emit` prints to stdout, so it takes no `-o`.
#[test]
fn emit_should_exit_2_when_the_command_also_has_an_output_flag() {
    assert_usage_errors(&[&["build", "--emit", "ast", "f.sisu", "-o", "x"]]);
}

/// Runs `sisu <args>` in the target tmp dir.
fn sisu_in_tmp(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(env!("CARGO_TARGET_TMPDIR"))
        .args(args)
        .output()
        .expect("sisu starts")
}

/// GIVEN programs given with `-e`, one valid and one with a warning
/// WHEN `sisu check -e` runs
/// THEN the valid program exits 0 with empty stderr, and the warning names `<inline>` at its
/// position
///
/// `check -e` reports a program given on the command line, named `<inline>`.
#[test]
fn check_should_report_an_inline_program_under_the_name_inline() {
    let out = sisu_in_tmp(&["check", "-e", "fn main() { print(1) }"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");

    let out = sisu_in_tmp(&["check", "-e", "fn main() { let mut x = 1; print(x) }"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.starts_with(
            "warning[W0002]: `x` is never reassigned\n --> <inline>:1:21\n  |\n\
             1 | fn main() { let mut x = 1; print(x) }\n"
        ),
        "{stderr}"
    );
}

/// GIVEN an inline program with an error
/// WHEN `sisu check --format json -e` runs
/// THEN it exits 1 and the file of the first record is `<inline>`
#[test]
fn check_should_name_the_file_inline_in_json_diagnostics_for_an_inline_program() {
    let out = sisu_in_tmp(&["check", "--format", "json", "-e", "fn main() { x }"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let first = stderr.lines().next().expect("a diagnostic");
    let record: serde_json::Value = serde_json::from_str(first).expect("JSON");
    assert_eq!(record["primary"]["file"], "<inline>", "{stderr}");
}

/// GIVEN an inline program that prints 7
/// WHEN `sisu build -e` runs with `-o`, and with `--emit tir`
/// THEN the executable prints 7, and the TIR is the same as the TIR of a file with that program
#[test]
fn build_should_compile_an_inline_program_like_the_same_file() {
    let dir = scratch_path::empty_dir("build_takes_an_inline_program");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(&dir)
        .args(["build", "-e", "fn main() { print(7) }", "-o", "e"])
        .output()
        .expect("sisu starts");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(dir.join("e"))
        .output()
        .expect("the program starts");
    assert_eq!(String::from_utf8_lossy(&run.stdout), "7\n");

    let file = dir.join("p.sisu");
    fs::write(&file, "fn main() { print(7) }").expect("the program should be written");
    let from_file = sisu_in_tmp(&["build", "--emit", "tir", file.to_str().expect("UTF-8 path")]);
    let inline = sisu_in_tmp(&["build", "--emit", "tir", "-e", "fn main() { print(7) }"]);
    assert_eq!(inline.status.code(), Some(0));
    assert_ne!(inline.stdout, Vec::<u8>::new());
    assert_eq!(inline.stdout, from_file.stdout);
}

/// GIVEN an inline program and no `-o`
/// WHEN `sisu build -e` runs
/// THEN it exits 2 and the error says an inline program needs `-o <output>`
///
/// An inline program has no name to give an executable.
#[test]
fn build_should_exit_2_when_an_inline_program_has_no_output_path() {
    let stderr = assert_usage_errors(&[&["build", "-e", "fn main() {}"]]);
    assert!(
        stderr.starts_with("error: an inline program needs `-o <output>` to build an executable\n"),
        "{stderr}"
    );
}

/// GIVEN `-e` together with a file, `-e` twice, or `-e` with no value
/// WHEN `sisu` runs
/// THEN it exits 2 with a usage error
///
/// A file and `-e` exclude each other, and `-e` takes one program.
#[test]
fn inline_flag_should_exit_2_when_given_with_a_file_twice_or_without_a_value() {
    assert_usage_errors(&[
        &["check", "f.sisu", "-e", "fn main() {}"],
        &["build", "f.sisu", "-e", "fn main() {}", "-o", "x"],
        &["check", "-e", "a", "-e", "b"],
        &["check", "-e"],
    ]);
}

/// GIVEN an inline program that reads an array out of bounds
/// WHEN it is built with `-e` and run
/// THEN it exits 101 and stderr starts with `sisu: panic at <inline>:1:`
#[test]
fn built_inline_program_should_name_inline_in_its_panic_message() {
    let dir = scratch_path::empty_dir("inline_panic_names_inline");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(&dir)
        .args([
            "build",
            "-e",
            "fn main() { let a = [1]; print(a[3]) }",
            "-o",
            "e",
        ])
        .output()
        .expect("sisu starts");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(dir.join("e"))
        .output()
        .expect("the program starts");
    assert_eq!(run.status.code(), Some(101));
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.starts_with("sisu: panic at <inline>:1:"), "{stderr}");
}

/// GIVEN an inline program with an invalid byte
/// WHEN `sisu check -e` runs
/// THEN it exits 1 and reports E0072 at `<inline>:1:13`
#[test]
fn check_should_report_e0072_for_an_inline_program_that_is_not_utf8() {
    use std::os::unix::ffi::OsStrExt;

    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .args(["check", "-e"])
        .arg(std::ffi::OsStr::from_bytes(b"fn main() { \xff }"))
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("error[E0072]: source is not valid UTF-8\n --> <inline>:1:13\n"),
        "{stderr}"
    );
}

/// `sisu run`, run from `tests/` with `TMPDIR` set to `tmp`, where it makes its directory.
fn sisu_run(tmp: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sisu"));
    command
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests"))
        .env("TMPDIR", tmp)
        .arg("run");
    command
}

/// Asserts that `sisu run` left nothing in `tmp`.
fn assert_empty(tmp: &Path) {
    let left: Vec<_> = fs::read_dir(tmp)
        .expect("reads the temp directory")
        .map(|entry| entry.expect("reads an entry").file_name())
        .collect();
    assert!(left.is_empty(), "left in {}: {left:?}", tmp.display());
}

/// GIVEN a program that echoes its arguments and its input
/// WHEN `sisu run` runs it with arguments, with input on stdin, and with `--`
/// THEN the output is the expected output, stderr is empty, a first `--` is dropped, a later `--`
/// passes
/// on, and the temporary directory is empty
#[test]
fn run_should_pass_stdin_and_arguments_to_the_program_and_leave_no_directory() {
    let tmp = scratch_path::empty_dir("run_passes_stdin_and_arguments");
    let case = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/run"));
    let mut child = sisu_run(&tmp)
        .args(["run/args_and_stdin.sisu", "a", "-x", "--flag"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("sisu starts");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(&fs::read(case.join("args_and_stdin.in")).expect("reads"))
        .expect("writes stdin");
    let out = child.wait_with_output().expect("sisu runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stderr, "");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        fs::read_to_string(case.join("args_and_stdin.out")).expect("reads")
    );
    // A first `--` is dropped, a later one passed on.
    for (args, expected) in [(&["--", "a"][..], "a\n"), (&["a", "--", "b"], "a\n--\nb\n")] {
        let out = sisu_run(&tmp)
            .arg("run/args_and_stdin.sisu")
            .args(args)
            .output()
            .expect("sisu starts");
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), expected, "{args:?}");
    }
    assert_empty(&tmp);
}

/// GIVEN a program that ends well, one that overflows the stack, and one that panics
/// WHEN `sisu run` runs each
/// THEN the exit codes are 0, 139 and 101, a panic prints its output and message, and the temporary
/// directory is empty
#[test]
fn run_should_return_the_exit_status_of_the_program_and_leave_no_directory() {
    let tmp = scratch_path::empty_dir("run_passes_the_exit_code");
    let out = sisu_run(&tmp)
        .arg("run/args_none.sisu")
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "0\n");
    // Without end, the recursion overflows the stack: SIGSEGV, 11, as a shell reports it.
    let out = sisu_run(&tmp)
        .args([
            "-e",
            "fn f(n: i64) -> i64 { f(n + 1) + 1 }\nfn main() { print(f(0)) }",
        ])
        .output()
        .expect("sisu starts");
    assert_eq!(
        out.status.code(),
        Some(128 + 11),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run_fail = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/run-fail"));
    let out = sisu_run(&tmp)
        .current_dir(run_fail)
        .arg("panic.sisu")
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(101));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n");
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        fs::read_to_string(run_fail.join("panic.err")).expect("reads")
    );
    assert_empty(&tmp);
}

/// GIVEN the program `E0001.sisu`, which has a compile error
/// WHEN `sisu run` runs it
/// THEN it exits 1, stdout is empty, stderr is the blessed diagnostic, and the temporary directory
/// is
/// empty
#[test]
fn run_should_run_nothing_and_exit_1_when_the_program_has_a_compile_error() {
    let tmp = scratch_path::empty_dir("run_with_a_compile_error_runs_nothing");
    let diagnostics = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/diagnostics"));
    let out = sisu_run(&tmp)
        .current_dir(diagnostics)
        .arg("E0001.sisu")
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        fs::read_to_string(diagnostics.join("E0001.err")).expect("reads")
    );
    assert_empty(&tmp);
}

/// GIVEN a valid program
/// WHEN `sisu run -O2 --rc-checks` runs it
/// THEN it exits 0, prints 0, and leaves the temporary directory empty
#[test]
fn run_should_accept_build_flags_before_the_file() {
    let tmp = scratch_path::empty_dir("run_takes_build_flags");
    let out = sisu_run(&tmp)
        .args(["-O2", "--rc-checks", "run/args_none.sisu"])
        .output()
        .expect("sisu starts");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "0\n");
    assert_empty(&tmp);
}

/// GIVEN `MIMALLOC_VERBOSE=1`, and `--allocator system` or no allocator flag
/// WHEN `sisu run -e` runs
/// THEN the default run prints `mimalloc:` lines to stderr, the `system` run prints none, and both
/// print 1
///
/// `sisu run` links the runtime `--allocator` names: mimalloc by default, which writes
/// `mimalloc:` lines to stderr under `MIMALLOC_VERBOSE=1`, and with `system` a runtime that
/// holds no mimalloc.
#[test]
fn run_should_link_the_runtime_that_the_allocator_flag_names() {
    let tmp = scratch_path::empty_dir("run_takes_the_allocator_flag");
    for (flags, mimalloc) in [(&[][..], true), (&["--allocator", "system"], false)] {
        let out = sisu_run(&tmp)
            .env("MIMALLOC_VERBOSE", "1")
            .args(flags)
            .args(["-e", "fn main() {\n    print(1)\n}\n"])
            .output()
            .expect("sisu starts");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{flags:?}: {stderr}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n", "{flags:?}");
        assert_eq!(
            stderr.contains("mimalloc:"),
            mimalloc,
            "{flags:?}: {stderr}"
        );
    }
    assert_empty(&tmp);
}

/// GIVEN a program with a warning that then panics
/// WHEN `sisu run -e` runs it
/// THEN it exits 101, stderr has the warning and the summary once, and then only the panic of the
/// program
///
/// The warnings and their summary are on stderr before the program starts, and the summary
/// is not written again after it ends.
#[test]
fn run_should_print_the_warnings_and_summary_before_the_program_starts() {
    let tmp = scratch_path::empty_dir("run_prints_warnings_first");
    let out = sisu_run(&tmp)
        .args(["-e", "fn main() { let mut x = 1; panic(\"x is {x}\") }"])
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(101));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let summary = "\nsisu: 1 warning\nsisu: for more on a code, run `sisu explain <code>`: W0002\n";
    let (warning, panic) = stderr.split_once(summary).expect("one summary");
    assert!(warning.starts_with("warning[W0002]: "), "{stderr}");
    assert!(panic.starts_with("sisu: panic at <inline>:1:"), "{stderr}");
    assert_eq!(
        panic.matches("sisu: ").count(),
        1,
        "only the program's own panic follows the summary: {stderr}"
    );
    assert_empty(&tmp);
}

/// GIVEN an inline program that prints its arguments, and one argument
/// WHEN `sisu run -e` runs
/// THEN it exits 0 and prints the argument
#[test]
fn run_should_pass_the_arguments_after_an_inline_program_to_main() {
    let tmp = scratch_path::empty_dir("run_takes_an_inline_program");
    let out = sisu_run(&tmp)
        .args([
            "-e",
            "fn main(args: [string]) { for a in args { print(a) } }",
            "arg",
        ])
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "arg\n");
    assert_empty(&tmp);
}

/// GIVEN an inline program that prints its argument count, then `-O2` and `x`
/// WHEN `sisu run -e` runs
/// THEN it prints 2, so the program gets `-O2` as an argument
///
/// As with `python -c`, every argument after the code goes to the program, flags included.
#[test]
fn run_should_pass_flags_after_inline_code_to_the_program() {
    let tmp = scratch_path::empty_dir("run_passes_flags_after_inline_code");
    let out = sisu_run(&tmp)
        .args([
            "-e",
            "fn main(args: [string]) { print(args.len()) }",
            "-O2",
            "x",
        ])
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "2\n");
    assert_empty(&tmp);
}

/// GIVEN a program that prints forever, and a reader that leaves after one line
/// WHEN `sisu run -e` runs it
/// THEN it exits with 141, stderr is empty, and the temporary directory is empty
///
/// `sisu run x | head -1`: the program dies of SIGPIPE, as it would run directly, and `sisu`
/// says nothing of its own.
#[test]
fn run_should_let_the_program_die_of_sigpipe_when_stdout_closes() {
    use std::io::{BufRead, BufReader};

    let tmp = scratch_path::empty_dir("run_into_a_closed_pipe");
    let mut child = sisu_run(&tmp)
        .args(["-e", "fn main() { loop { print(1) } }"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("sisu starts");
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("stdout is piped"))
        .read_line(&mut line)
        .expect("reads a line");
    assert_eq!(line, "1\n");
    let out = child.wait_with_output().expect("sisu runs");
    assert_eq!(out.status.code(), Some(128 + 13));
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    assert_empty(&tmp);
}

/// GIVEN two `sisu run` processes of one file at the same time
/// WHEN both run to the end
/// THEN each exits 0 and prints 0, and the temporary directory is empty
///
/// Two `run`s of one file at once each get a directory of their own.
#[test]
fn run_should_give_two_runs_of_one_file_a_directory_each() {
    let tmp = scratch_path::empty_dir("two_runs_at_once");
    let start = || {
        sisu_run(&tmp)
            .arg("run/args_none.sisu")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("sisu starts")
    };
    for child in [start(), start()] {
        let out = child.wait_with_output().expect("sisu runs");
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout), "0\n");
    }
    assert_empty(&tmp);
}

/// GIVEN the program `E0001.sisu`, which has a compile error
/// WHEN `sisu run --format json` runs it
/// THEN it exits 1, stdout is empty, stderr is a diagnostic record and a summary, and the temporary
/// directory is empty
#[test]
fn run_with_json_format_should_run_nothing_when_the_program_has_an_error() {
    let tmp = scratch_path::empty_dir("json_run_with_an_error");
    let out = sisu_run(&tmp)
        .args(["--format", "json", "diagnostics/E0001.sisu"])
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let kinds: Vec<String> = stderr
        .lines()
        .map(|l| {
            let record: serde_json::Value =
                serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}"));
            record["kind"].as_str().expect("a kind").to_string()
        })
        .collect();
    assert_eq!(kinds, ["diagnostic", "summary"], "{stderr}");
    assert_empty(&tmp);
}

/// GIVEN `run` with no file, and `run -O2` with no file
/// WHEN `sisu` runs
/// THEN it exits 2 with a usage error
#[test]
fn run_should_exit_2_when_no_program_is_given() {
    assert_usage_errors(&[&["run"], &["run", "-O2"]]);
}

/// GIVEN a program that waits for a line on stdin
/// WHEN `sisu run -e` has started it
/// THEN the temporary directory is empty before the program ends, and the program then prints its
/// line
/// and exits 0
///
/// The program's directory is gone while it still runs: `sisu` removes it before it waits.
#[test]
fn run_should_remove_its_directory_while_the_program_still_runs() {
    use std::io::{BufRead, BufReader};
    use std::time::{Duration, Instant};

    let tmp = scratch_path::empty_dir("run_removes_its_directory");
    // `eprint`, not `print`: stdout into a pipe is block-buffered, and `read_line` does not
    // flush it.
    let mut child = sisu_run(&tmp)
        .args([
            "-e",
            "fn main() { eprint(\"ready\"); print(read_line() ?? \"none\") }",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("sisu starts");
    let mut stderr = BufReader::new(child.stderr.take().expect("stderr is piped"));
    let mut line = String::new();
    stderr.read_line(&mut line).expect("reads a line");
    assert_eq!(line, "ready\n");
    // A hang guard, not a measure of speed.
    let deadline = Instant::now() + Duration::from_secs(30);
    while fs::read_dir(&tmp).expect("reads").next().is_some() {
        assert!(
            Instant::now() < deadline,
            "the directory is still there while the program waits for input"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(b"line\n")
        .expect("writes stdin");
    let out = child.wait_with_output().expect("sisu runs");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "line\n");
}

/// GIVEN a `PATH` with no `cc`
/// WHEN `sisu run` runs
/// THEN it exits 1, stdout is empty, stderr says it cannot run `cc`, and the temporary directory is
/// empty
///
/// A link that fails, here because `cc` is not on `PATH`, leaves nothing behind.
#[test]
fn run_should_exit_1_and_leave_nothing_when_the_link_fails() {
    let tmp = scratch_path::empty_dir("run_link_failure_leaves_nothing");
    let out = sisu_run(&tmp)
        .env("PATH", scratch_path::empty_dir("run_link_failure_path"))
        .arg("run/args_none.sisu")
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("sisu: cannot run cc: "), "{stderr}");
    assert_empty(&tmp);
}

/// GIVEN a `TMPDIR` that does not exist
/// WHEN `sisu run` and `sisu build` run
/// THEN each exits 1 and stderr says it cannot create its file in that directory
///
/// A temp directory that does not exist is a tool error: `run` cannot make its directory there,
/// and `build` cannot write its object file.
#[test]
fn run_and_build_should_exit_1_when_the_temporary_directory_is_missing() {
    let dir = scratch_path::empty_dir("missing_temp_dir");
    let absent = dir.join("absent");
    let output = dir.join("out");
    let output = output.to_str().expect("the scratch path should be UTF-8");
    for (args, suffix) in [
        (&["run", "run/args_none.sisu"][..], ""),
        (&["build", "run/args_none.sisu", "-o", output], ".o"),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests"))
            .env("TMPDIR", &absent)
            .args(args)
            .output()
            .expect("sisu starts");
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "", "{args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        let expected = format!("sisu: cannot create {}/sisu-", absent.display());
        assert!(stderr.starts_with(&expected), "{args:?}: {stderr}");
        assert!(
            stderr.ends_with(&format!(
                "{suffix}: No such file or directory (os error 2)\n"
            )),
            "{args:?}: {stderr}"
        );
    }
}

/// GIVEN a umask that takes the write bit of the owner
/// WHEN `sisu run -e` runs
/// THEN it exits 0, prints 42, and leaves the temporary directory empty
///
/// Under a umask that takes the owner's write bit, `run` still makes its directory the
/// owner's, runs the program and leaves nothing behind. `umask 0777` shows the same bug but
/// would make the coverage run's own profile file unreadable.
#[test]
fn run_should_succeed_and_leave_nothing_under_a_restrictive_umask() {
    let tmp = scratch_path::empty_dir("run_under_a_restrictive_umask");
    let out = Command::new("sh")
        .args(["-c", "umask 0200; exec \"$@\"", "sh"])
        .arg(env!("CARGO_BIN_EXE_sisu"))
        .args(["run", "-e", "fn main() { print(42) }"])
        .env("TMPDIR", &tmp)
        .output()
        .expect("sh starts");
    // Unlocks what a failure leaves, which the next run could not remove.
    assert_eq!(unlock_and_empty(&tmp), 0);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "42\n");
}

/// A program with one warning, so text output has a summary.
const WARNS: &str = "fn main() { let mut x = 1; print(x) }";

/// The summary `WARNS` gets.
const WARNS_SUMMARY: &str =
    "sisu: 1 warning\nsisu: for more on a code, run `sisu explain <code>`: W0002\n";

/// A `PATH` whose `cc` the environment variable `FAKE_CC` steers, to make `run` fail where it
/// otherwise cannot:
/// - `no-output` writes no executable;
/// - `lock-parent` links with the real `cc`, then takes write permission from the temp dir,
///   so the private directory, empty by then, cannot be removed;
/// - `lock-dir-and-fail` takes write permission from the private directory and fails.
fn fake_cc(name: &str) -> std::ffi::OsString {
    use std::os::unix::fs::PermissionsExt;

    let dir = scratch_path::empty_dir(name);
    let path = std::env::var_os("PATH").unwrap_or_default();
    let real = std::env::split_paths(&path)
        .map(|dir| dir.join("cc"))
        .find(|cc| cc.is_file())
        .expect("cc is on PATH");
    let script = format!(
        "#!/bin/sh\n\
         for arg; do out=$arg; done\n\
         dir=$(dirname \"$out\")\n\
         case $FAKE_CC in\n\
         no-output) exit 0 ;;\n\
         lock-parent) '{}' \"$@\" || exit; chmod 500 \"$(dirname \"$dir\")\" ;;\n\
         lock-dir-and-fail) chmod 500 \"$dir\"; exit 1 ;;\n\
         esac\n",
        real.display()
    );
    let cc = dir.join("cc");
    fs::write(&cc, script).expect("writes");
    fs::set_permissions(&cc, fs::Permissions::from_mode(0o755)).expect("makes it executable");
    std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&path)))
        .expect("PATH joins")
}

/// GIVEN a fake `cc` that writes no executable, and a program with a warning
/// WHEN `sisu run` runs, in text format and in JSON format
/// THEN it exits 1 and reports a tool error after the summary, as one text line and as a JSON
/// record
/// AND the temporary directory is empty
///
/// A program that cannot start is a tool error after the summary, in text and in JSON, and
/// its directory is gone.
#[test]
fn run_should_report_a_program_that_cannot_start_after_the_summary() {
    let tmp = scratch_path::empty_dir("run_program_cannot_start");
    let path = fake_cc("run_program_cannot_start_cc");
    let prefix = format!("cannot run {}/sisu-", tmp.display());
    let suffix = "/program: No such file or directory (os error 2)";
    let out = sisu_run(&tmp)
        .env("PATH", &path)
        .env("FAKE_CC", "no-output")
        .args(["-e", WARNS])
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let (_, after) = stderr.split_once(WARNS_SUMMARY).expect("a summary");
    let error = after
        .strip_prefix(&format!("sisu: {prefix}"))
        .unwrap_or_else(|| panic!("{stderr}"));
    assert!(error.ends_with(&format!("{suffix}\n")), "{stderr}");
    assert_eq!(after.lines().count(), 1, "{stderr}");

    let out = sisu_run(&tmp)
        .env("PATH", &path)
        .env("FAKE_CC", "no-output")
        .args(["--format", "json", "-e", WARNS])
        .output()
        .expect("sisu starts");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let records: Vec<serde_json::Value> = stderr
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
        .collect();
    let kinds: Vec<&str> = records
        .iter()
        .map(|r| r["kind"].as_str().expect("a kind"))
        .collect();
    assert_eq!(kinds, ["diagnostic", "summary", "tool_error"], "{stderr}");
    let message = records[2]["message"].as_str().expect("a message");
    assert!(
        message.starts_with(&prefix) && message.ends_with(suffix),
        "{message}"
    );
    assert_empty(&tmp);
}

/// Gives the owner back write permission on `tmp` and what is in it, removes what is in it,
/// and returns how many entries there were.
fn unlock_and_empty(tmp: &Path) -> usize {
    use std::os::unix::fs::PermissionsExt;

    let writable = || fs::Permissions::from_mode(0o700);
    fs::set_permissions(tmp, writable()).expect("unlocks the temp dir");
    let entries: Vec<_> = fs::read_dir(tmp)
        .expect("reads the temp dir")
        .map(|entry| entry.expect("reads an entry").path())
        .collect();
    for entry in &entries {
        fs::set_permissions(entry, writable()).expect("unlocks the entry");
        fs::remove_dir_all(entry).expect("removes the entry");
    }
    entries.len()
}

/// GIVEN a fake `cc` that takes the write permission from the temporary directory, or from the
/// private
/// directory and fails
/// WHEN `sisu run` runs
/// THEN a started program keeps its exit code and the tool error follows the summary
/// AND after a failed link the tool error comes before the summary, after the error of the link
///
/// A private directory that cannot be removed is a tool error: after the summary once the
/// program has started, which still runs and keeps its exit code; before it after a failed
/// link, after the link's own error.
#[test]
fn run_should_report_a_directory_it_cannot_remove_as_a_tool_error() {
    use std::os::unix::fs::MetadataExt;

    let tmp = scratch_path::empty_dir("run_directory_cannot_be_removed");
    if fs::metadata(&tmp).expect("reads metadata").uid() == 0 {
        eprintln!("skipped: root can remove a directory without write permission");
        return;
    }
    let path = fake_cc("run_directory_cannot_be_removed_cc");
    let cannot_remove = format!("sisu: cannot remove {}/sisu-", tmp.display());
    let denied = ": Permission denied (os error 13)";

    let out = sisu_run(&tmp)
        .env("PATH", &path)
        .env("FAKE_CC", "lock-parent")
        .args(["-e", WARNS])
        .output()
        .expect("sisu starts");
    assert_eq!(unlock_and_empty(&tmp), 1);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n");
    let (_, after) = stderr.split_once(WARNS_SUMMARY).expect("a summary");
    assert!(after.starts_with(&cannot_remove), "{stderr}");
    assert!(after.ends_with(&format!("{denied}\n")), "{stderr}");
    assert_eq!(after.lines().count(), 1, "{stderr}");

    let out = sisu_run(&tmp)
        .env("PATH", &path)
        .env("FAKE_CC", "lock-dir-and-fail")
        .args(["-e", WARNS])
        .output()
        .expect("sisu starts");
    assert_eq!(unlock_and_empty(&tmp), 1);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let before = stderr
        .strip_suffix(WARNS_SUMMARY)
        .expect("the summary last");
    let tool_errors: Vec<&str> = before.lines().rev().take(2).collect();
    assert_eq!(
        tool_errors[1], "sisu: cc failed: exit status: 1",
        "{stderr}"
    );
    assert!(
        tool_errors[0].starts_with(&cannot_remove) && tool_errors[0].ends_with(denied),
        "{stderr}"
    );
}
