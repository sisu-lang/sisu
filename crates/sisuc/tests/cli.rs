//! End to end: the `sisu` command line.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

mod scratch_path;

#[test]
fn emit_tokens() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("emit_tokens.sisu"), "fn main() {}\n").expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["build", "--emit", "tokens", "emit_tokens.sisu"])
        .output()
        .expect("sisu starts");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("1:1 Fn fn\n1:4 Ident main\n"));
}

#[test]
fn emit_tokens_names_the_class_and_optional_tokens() {
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

#[test]
fn emit_tokens_char() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("emit_tokens_char.sisu"), "'ä' '\\n'\n").expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["build", "--emit", "tokens", "emit_tokens_char.sisu"])
        .output()
        .expect("sisu starts");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("1:1 Char 'ä'\n1:5 Char '\\n'\n"),
        "{stdout}"
    );
}

#[test]
fn emit_tokens_float() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("emit_tokens_float.sisu"), "1.5e3\n").expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["build", "--emit", "tokens", "emit_tokens_float.sisu"])
        .output()
        .expect("sisu starts");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "1:1 Float 1.5e3\n1:6 Newline\n1:6 Eof\n"
    );
}

#[test]
fn emit_tokens_star_star() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("emit_tokens_star_star.sisu"), "x ** 2\n").expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["build", "--emit", "tokens", "emit_tokens_star_star.sisu"])
        .output()
        .expect("sisu starts");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "1:1 Ident x\n1:3 StarStar **\n1:6 Int 2\n1:7 Newline\n1:7 Eof\n"
    );
}

#[test]
fn emit_tokens_todo() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("emit_tokens_todo.sisu"), "todo\n").expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["build", "--emit", "tokens", "emit_tokens_todo.sisu"])
        .output()
        .expect("sisu starts");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "1:1 Todo todo\n1:5 Newline\n1:5 Eof\n"
    );
}

#[test]
fn emit_tokens_strings() {
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

#[test]
fn emit_tokens_reserved() {
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

#[test]
fn lexer_error_exits_1() {
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

/// A source the lexer cannot finish (a stray bracket) fails `--emit tokens` and `--emit ast`.
#[test]
fn unlexable_source_exits_1_for_tokens_and_ast() {
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

/// No arguments, `help`, `-h` and `--help` print the usage to stdout and exit 0.
#[test]
fn help_and_no_arguments_exit_0() {
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

/// Help, version, an explanation and an emitted stage that cannot be written are a tool error,
/// not a panic or a silent success.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "writes to /dev/full, which only Linux has"
)]
fn output_into_a_full_stdout_is_a_tool_error() {
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

/// A reader that has already left is a quiet end, not an error: `sisu --help | head -1`. The read
/// end is closed before `sisu` starts, so the write fails every time.
#[test]
fn output_into_a_closed_pipe_is_quiet() {
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

/// A wrong command line exits 2, and a broken flag rule names the flag with `build`'s usage.
#[test]
fn usage_errors_exit_2() {
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

/// A subcommand with too few arguments, or with the arguments of another one.
#[test]
fn mode_with_the_wrong_arguments_is_a_usage_error() {
    assert_usage_errors(&[&["check"], &["check", "tir", "f.sisu"]]);
}

/// `--version` and `-V` print the name and the version, and nothing else.
#[test]
fn version_prints_the_version() {
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

/// `--version --verbose` adds the commit, the host and the LLVM version; each line is checked
/// by shape, since the values change with the checkout and the machine.
#[test]
fn verbose_version_prints_four_lines() {
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

/// `--verbose` means something only with `--version`; `--version` takes no subcommand.
#[test]
fn verbose_alone_is_a_usage_error() {
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

/// `--rc-checks` goes only with a build or IR, once.
#[test]
fn rc_checks_is_build_or_ir_only() {
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

/// `--rc-checks` with `--emit ir-raw` and `--emit ir` prints the module with the count checks
/// built in, so a refactor can compare that IR too; without it, the module has none.
#[test]
fn emit_ir_with_rc_checks() {
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

#[test]
fn emit_ast() {
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

#[test]
fn emit_ast_keeps_compound_assign() {
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

#[test]
fn emit_ast_and_tir_print_mut() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(
        dir.join("emit_mut.sisu"),
        "class P {\n    name: i64\n    mut hp: i64\n}\nfn main() {\n    let mut x = 1\n    x = 2\n}\n",
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
        "(class P (name i64) (mut hp i64))\n(fn main () unit (block (let mut x 1) (= x 2)))\n"
    );
    assert_eq!(
        emit("tir"),
        "(class P (name i64) (mut hp i64))\n(fn main () unit (block (let mut x#0 1) (= x#0 2)))\n"
    );
}

#[test]
fn parse_error_exits_1() {
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

#[test]
fn check_valid_file() {
    let out = check("check_valid_file", "fn main() {\n    print(1)\n}\n");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
}

#[test]
fn check_warning_exits_0() {
    let out = check(
        "check_warning_exits_0",
        "fn main() {\n    let mut count = 0\n    print(count)\n}\n",
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("warning[W0002]: "));
}

#[test]
fn check_error_exits_1() {
    let out = check(
        "check_error_exits_1",
        "fn main() {\n    let n = 0\n    n = 1\n}\n",
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("error[E0001]: cannot assign to `n`"));
}

#[test]
fn check_warning_and_error_exits_1() {
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

#[test]
fn explain_prints_the_explanation() {
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

/// A retired code's page is still served, in either case, and says it is retired.
#[test]
fn explain_prints_a_retired_explanation() {
    for code in ["E0012", "e0012"] {
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

#[test]
fn explain_rejects_an_unknown_code() {
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

#[test]
fn unreadable_input_exits_1() {
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

#[test]
fn oversized_input_is_a_tool_error() {
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

#[test]
fn check_reads_a_pipe() {
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

#[test]
fn invalid_utf8_on_line_two_is_a_diagnostic_at_its_line_and_column() {
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

#[test]
fn invalid_utf8_at_the_start_is_a_diagnostic() {
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

#[test]
fn a_truncated_character_at_end_of_file_is_a_diagnostic() {
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

#[test]
fn invalid_utf8_after_crlf_line_breaks_counts_each_break_as_one_line() {
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

#[test]
fn an_encoded_surrogate_is_invalid_utf8_at_its_first_byte() {
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

#[test]
fn mem2reg_removes_every_alloca() {
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

#[test]
fn emit_ir_error_prints_no_ir() {
    let out = run_on(
        "emit_ir_error_prints_no_ir",
        "fn main() {\n    let n = 0\n    n = 1\n}\n",
        &["build", "--emit", "ir"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("error[E0001]: cannot assign to `n`"));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
}

#[test]
fn emit_ir_warning_still_prints_ir() {
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

/// A warning does not stop `--emit tokens` or `--emit ast`, as it does not stop `check`.
#[test]
fn emit_with_a_redundant_semicolon_still_prints() {
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

#[test]
fn emit_tir() {
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

#[test]
fn emit_mir_shows_moves() {
    let out = run_on(
        "emit_mir_shows_moves",
        "fn f(xs: [i64]) -> [i64] { xs }\nfn main() {}\n",
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

/// `--emit mir` prints every function's blocks.
#[test]
fn emit_mir() {
    let out = run_on(
        "emit_mir",
        "enum E {\n    A(string)\n    B\n}\n\nfn name(e: E) -> string {\n    match e {\n        A(s) => s\n        B => \"b\"\n    }\n}\n\nfn main() {\n    print(name(E.A(\"a\")))\n}\n",
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

/// A stage `--emit` does not know is a usage error, and clap's error lists `mir` among the stages.
#[test]
fn emit_unknown_stage_lists_mir() {
    let stderr = assert_usage_errors(&[&["build", "--emit", "bogus", "f.sisu"]]);
    assert!(
        stderr.contains("[possible values: tokens, ast, tir, mir, ir, ir-raw]"),
        "{stderr}"
    );
}

#[test]
fn emit_with_an_error_prints_nothing_on_stdout() {
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

/// On a valid file, `--format json` leaves stdout as text has it, and stderr holds only the
/// summary.
#[test]
fn json_emit_keeps_stdout() {
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

#[test]
fn o2_combinations_are_usage_errors() {
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

/// `-O3` takes `-O2`'s place, once, and `--linker` goes with a build only, once.
#[test]
fn o3_and_linker_combinations_are_usage_errors() {
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

/// `--allocator` names one of the two runtimes, and goes with a build only, once: neither
/// runtime changes the emitted IR, so even the default, `mimalloc`, is refused with `--emit`.
#[test]
fn allocator_goes_with_a_build_only() {
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

#[test]
fn emit_ir_raw_parses_back() {
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

#[test]
fn emit_ir_raw_is_the_ir_before_mem2reg() {
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

#[test]
fn emit_ir_o2() {
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

/// Only `default<O3>` unswitches a loop whose branch does work on both sides: it splits the
/// loop into one per side, so the program has more loops than at `default<O2>`.
#[test]
fn emit_ir_o3_runs_the_o3_pipeline() {
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

/// A `print` that may never run does not make the loop reload the array's length: the
/// runtime's print functions write only memory the module cannot reach, so `-O2` proves
/// every `xs[i]` in range from the loop bound and drops both bounds checks.
#[test]
fn print_in_a_loop_keeps_no_bounds_check_at_o2() {
    let source = "fn main() {\n    let xs = [1; 10_000]\n    let mut total = 0\n\
                  for round in 0..1000 {\n        for i in 0..10_000 {\n\
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

/// A `for ..=` loop steps its counter only while it is below the end, so the step needs no
/// overflow check, and `-O2` folds a loop over constant bounds as it does over `..`.
#[test]
fn inclusive_for_folds_at_o2() {
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

#[test]
fn json_unreadable_input_is_a_tool_error() {
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

/// `--format` takes `text` or `json`, and never goes with `explain`. A usage error is clap's
/// text, also with `--format json`.
#[test]
fn format_usage_errors() {
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

#[test]
fn json_link_failure_stays_json() {
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

#[test]
fn json_overwrite_is_a_tool_error() {
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

#[test]
fn warnings_only_build_succeeds_with_summary() {
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

#[test]
fn format_text_is_the_default() {
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

#[test]
fn emit_tokens_format_spec() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(
        dir.join("emit_tokens_format_spec.sisu"),
        "print(\"{x:>8.2}\")\n",
    )
    .expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["build", "--emit", "tokens", "emit_tokens_format_spec.sisu"])
        .output()
        .expect("sisu starts");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "1:1 Ident print\n1:6 LParen (\n1:7 StrHead \"{\n1:9 Ident x\n1:10 FormatSpec :>8.2\n1:15 StrTail }\"\n1:17 RParen )\n1:18 Newline\n1:18 Eof\n"
    );
}

#[test]
fn emit_tokens_enum() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("emit_tokens_enum.sisu"), "enum\n").expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["build", "--emit", "tokens", "emit_tokens_enum.sisu"])
        .output()
        .expect("sisu starts");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "1:1 Enum enum\n1:5 Eof\n"
    );
}

#[test]
fn emit_tokens_match() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("emit_tokens_match.sisu"), "match =>\n").expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["build", "--emit", "tokens", "emit_tokens_match.sisu"])
        .output()
        .expect("sisu starts");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "1:1 Match match\n1:7 FatArrow =>\n1:9 Eof\n"
    );
}

#[test]
fn emit_tokens_pipe_and_inclusive_range() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("emit_tokens_pipe.sisu"), "| ..= ||\n").expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(["build", "--emit", "tokens", "emit_tokens_pipe.sisu"])
        .output()
        .expect("sisu starts");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "1:1 Pipe |\n1:3 DotDotEq ..=\n1:7 OrOr ||\n1:9 Eof\n"
    );
}

#[test]
fn enum_layouts_follow_the_target() {
    use std::fmt::Write as _;
    // `Outer` is declared before the `Pair?` it holds and `Holder` before the `Trio` it holds,
    // so each layout needs a later one's first. `Wide` has 257 variants, one more than an `i8`
    // tag holds.
    let wide = (0..257).fold(String::new(), |mut wide, v| {
        writeln!(wide, "    V{v}").expect("writing to a `String` should not fail");
        wide
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
         }}\n"
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
    // payload, `{ i1, %sisu.Pair }` and `%sisu.Trio` at 32 bytes; no storage when no variant
    // has a field.
    for wanted in [
        "%sisu.Pair = type { i8, [2 x i64] }",
        "%sisu.Outer = type { i8, [4 x i64] }",
        "%sisu.Trio = type { i8, [3 x i64] }",
        "%sisu.Holder = type { i8, [4 x i64] }",
        "%sisu.Light = type { i8 }",
        "%sisu.Wide = type { i16 }",
    ] {
        assert!(ir.contains(wanted), "missing {wanted:?} in\n{ir}");
    }
}

/// A `sisu` with no runtime library beside it says so before it compiles: with no
/// temporary directory, a compile would fail at the object file first.
#[test]
fn missing_runtime_is_found_before_the_compile() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("missing_runtime");
    let _ = fs::remove_dir_all(&dir);
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

/// An output whose directory does not exist, or that is a directory, is reported before
/// the compile.
#[test]
fn bad_output_path_is_found_before_the_compile() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("bad_output_path");
    let _ = fs::remove_dir_all(&dir);
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

/// `build`'s flags go before or after the file, in any order. The `--linker` rows run on Linux
/// only: Apple's clang has no `bfd`, and for `lld` it looks for `ld64.lld`, which `.llvm/23` lacks.
#[test]
fn flags_in_any_order() {
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
        let _ = fs::remove_file(dir.join("e"));
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

/// Without `-o`, `build` names the executable after the file, without its extension, in the
/// current directory, wherever the file is.
#[test]
fn build_writes_the_stem_in_the_current_directory() {
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

/// `--format` follows the subcommand.
#[test]
fn format_after_the_subcommand() {
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

/// `--emit` prints to stdout, so it takes no `-o`.
#[test]
fn emit_with_output_is_a_usage_error() {
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

/// `check -e` reports a program given on the command line, named `<inline>`.
#[test]
fn check_takes_an_inline_program() {
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

#[test]
fn inline_program_in_json() {
    let out = sisu_in_tmp(&["check", "--format", "json", "-e", "fn main() { x }"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let first = stderr.lines().next().expect("a diagnostic");
    let record: serde_json::Value = serde_json::from_str(first).expect("JSON");
    assert_eq!(record["primary"]["file"], "<inline>", "{stderr}");
}

#[test]
fn build_takes_an_inline_program() {
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

/// An inline program has no name to give an executable.
#[test]
fn inline_build_needs_an_output() {
    let stderr = assert_usage_errors(&[&["build", "-e", "fn main() {}"]]);
    assert!(
        stderr.starts_with("error: an inline program needs `-o <output>` to build an executable\n"),
        "{stderr}"
    );
}

/// A file and `-e` exclude each other, and `-e` takes one program.
#[test]
fn file_and_inline_program_conflict() {
    assert_usage_errors(&[
        &["check", "f.sisu", "-e", "fn main() {}"],
        &["build", "f.sisu", "-e", "fn main() {}", "-o", "x"],
        &["check", "-e", "a", "-e", "b"],
        &["check", "-e"],
    ]);
}

#[test]
fn inline_panic_names_inline() {
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

#[test]
fn inline_program_not_utf8() {
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

#[test]
fn run_passes_stdin_and_arguments() {
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

#[test]
fn run_passes_the_exit_code() {
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

#[test]
fn run_with_a_compile_error_runs_nothing() {
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

#[test]
fn run_takes_build_flags() {
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

/// `sisu run` links the runtime `--allocator` names: mimalloc by default, which writes
/// `mimalloc:` lines to stderr under `MIMALLOC_VERBOSE=1`, and with `system` a runtime that
/// holds no mimalloc.
#[test]
fn run_takes_the_allocator_flag() {
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

/// The warnings and their summary are on stderr before the program starts, and the summary
/// is not written again after it ends.
#[test]
fn run_prints_warnings_first() {
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

#[test]
fn run_takes_an_inline_program() {
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

/// As with `python -c`, every argument after the code goes to the program, flags included.
#[test]
fn run_passes_flags_after_inline_code_to_the_program() {
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

/// `sisu run x | head -1`: the program dies of SIGPIPE, as it would run directly, and `sisu`
/// says nothing of its own.
#[test]
fn run_into_a_closed_pipe_dies_of_sigpipe() {
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

/// Two `run`s of one file at once each get a directory of their own.
#[test]
fn two_runs_at_once() {
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

#[test]
fn json_run_with_an_error_runs_nothing() {
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

#[test]
fn run_without_a_program_is_a_usage_error() {
    assert_usage_errors(&[&["run"], &["run", "-O2"]]);
}

/// The program's directory is gone while it still runs: `sisu` removes it before it waits.
#[test]
fn run_removes_its_directory_while_the_program_runs() {
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

/// A link that fails, here because `cc` is not on `PATH`, leaves nothing behind.
#[test]
fn run_link_failure_leaves_nothing() {
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

/// A temp directory that does not exist is a tool error: `run` cannot make its directory there,
/// and `build` cannot write its object file.
#[test]
fn missing_temp_dir_is_a_tool_error() {
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

/// Under a umask that takes the owner's write bit, `run` still makes its directory the
/// owner's, runs the program and leaves nothing behind. `umask 0777` shows the same bug but
/// would make the coverage run's own profile file unreadable.
#[test]
fn run_under_a_restrictive_umask_leaves_nothing() {
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

/// A program that cannot start is a tool error after the summary, in text and in JSON, and
/// its directory is gone.
#[test]
fn run_reports_a_program_that_cannot_start_after_the_summary() {
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

/// A private directory that cannot be removed is a tool error: after the summary once the
/// program has started, which still runs and keeps its exit code; before it after a failed
/// link, after the link's own error.
#[test]
fn run_reports_a_directory_it_cannot_remove() {
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
