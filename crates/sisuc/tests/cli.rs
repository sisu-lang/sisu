//! End to end: the `sisuc` command line.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

#[test]
fn emit_tokens() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("emit_tokens.sisu"), "fn main() {}\n").expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(["--emit", "tokens", "emit_tokens.sisu"])
        .output()
        .expect("sisuc starts");
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
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(["--emit", "tokens", "emit_new_tokens.sisu"])
        .output()
        .expect("sisuc starts");
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
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(["--emit", "tokens", "emit_tokens_char.sisu"])
        .output()
        .expect("sisuc starts");
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
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(["--emit", "tokens", "emit_tokens_float.sisu"])
        .output()
        .expect("sisuc starts");
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
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(["--emit", "tokens", "emit_tokens_star_star.sisu"])
        .output()
        .expect("sisuc starts");
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
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(["--emit", "tokens", "emit_tokens_todo.sisu"])
        .output()
        .expect("sisuc starts");
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
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(["--emit", "tokens", "emit_tokens_strings.sisu"])
        .output()
        .expect("sisuc starts");
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
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(["--emit", "tokens", "emit_tokens_reserved.sisu"])
        .output()
        .expect("sisuc starts");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("1:1 Reserved record\n"), "{stdout}");
    assert!(stdout.contains("\n2:1 Const const\n"), "{stdout}");
}

#[test]
fn lexer_error_exits_1() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("lexer_error_exits_1.sisu"), "a & b\n").expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(["--emit", "tokens", "lexer_error_exits_1.sisu"])
        .output()
        .expect("sisuc starts");
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
        let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
            .current_dir(dir)
            .args(["--emit", stage, "unlexable_source.sisu"])
            .output()
            .expect("sisuc starts");
        assert_eq!(out.status.code(), Some(1), "{stage}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            at_severity(&stderr, "error") == Some(0),
            "{stage}: {stderr}"
        );
    }
}

/// A mode flag with too few arguments, or with the arguments of another mode.
#[test]
fn mode_with_the_wrong_arguments_is_a_usage_error() {
    assert_usage_errors(&[&["--check"], &["--check", "tir", "f.sisu"]]);
}

/// Runs `sisuc` with each of `cases` in the target tmp dir, and checks that each is a usage
/// error.
fn assert_usage_errors(cases: &[&[&str]]) {
    for args in cases {
        let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
            .current_dir(env!("CARGO_TARGET_TMPDIR"))
            .args(*args)
            .output()
            .expect("sisuc starts");
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.starts_with("usage: sisuc"), "{args:?}: {stderr}");
    }
}

/// `--rc-checks` is accepted only right after an optional `-O2`, and only for a build or IR.
#[test]
fn rc_checks_is_build_or_ir_only() {
    let cases: [&[&str]; 9] = [
        &["--rc-checks", "--explain", "E0001"],
        &["--rc-checks", "--emit", "tir", "f.sisu"],
        &["--rc-checks", "--emit", "mir", "f.sisu"],
        &["--rc-checks", "--emit", "tokens", "f.sisu"],
        &["--rc-checks", "--check", "f.sisu"],
        &["--rc-checks", "-O2", "f.sisu", "out"],
        &["--emit", "ir", "--rc-checks"],
        &["--check", "--rc-checks"],
        &["--rc-checks", "--rc-checks", "f.sisu"],
    ];
    assert_usage_errors(&cases);
}

/// `--rc-checks` with `--emit ir-raw` and `--emit ir` prints the module with the count checks
/// built in, so a refactor can compare that IR too; without it, the module has none.
#[test]
fn emit_ir_with_rc_checks() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("emit_rc.sisu"), "fn main() {\n    print(1)\n}\n").expect("writes");
    let ir = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
            .current_dir(dir)
            .args(args)
            .arg("emit_rc.sisu")
            .output()
            .expect("sisuc starts");
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    for args in [
        &["--rc-checks", "--emit", "ir-raw"][..],
        &["--rc-checks", "--emit", "ir"],
        &["-O2", "--rc-checks", "--emit", "ir"],
    ] {
        assert!(ir(args).contains("@sisu_rc_check_exit"), "{args:?}");
    }
    assert!(!ir(&["--emit", "ir-raw"]).contains("@sisu_rc_check_exit"));
}

#[test]
fn emit_ast() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(
        dir.join("emit_ast.sisu"),
        "fn main() {\n    print(1 + 2)\n}\n",
    )
    .expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(["--emit", "ast", "emit_ast.sisu"])
        .output()
        .expect("sisuc starts");
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
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(["--emit", "ast", "emit_ast_compound.sisu"])
        .output()
        .expect("sisuc starts");
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
        let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
            .current_dir(dir)
            .args(["--emit", what, "emit_mut.sisu"])
            .output()
            .expect("sisuc starts");
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
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(["--emit", "ast", "parse_error_exits_1.sisu"])
        .output()
        .expect("sisuc starts");
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

/// Runs `sisuc <flags> <name>.sisu` on `source`, saved as `<name>.sisu` in the target tmp dir.
fn run_on(name: &str, source: &str, flags: &[&str]) -> Output {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let file = format!("{name}.sisu");
    fs::write(dir.join(&file), source).expect("writes");
    Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(flags)
        .arg(&file)
        .output()
        .expect("sisuc starts")
}

/// Runs `sisuc --check` on `source`, saved as `<name>.sisu` in the target tmp dir.
fn check(name: &str, source: &str) -> Output {
    run_on(name, source, &["--check"])
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
        let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
            .args(["--explain", code])
            .output()
            .expect("sisuc starts");
        assert_eq!(out.status.code(), Some(0), "{code}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), expected, "{code}");
        assert_eq!(String::from_utf8_lossy(&out.stderr), "", "{code}");
    }
}

/// A retired code's page is still served, in either case, and says it is retired.
#[test]
fn explain_prints_a_retired_explanation() {
    for code in ["E0012", "e0012"] {
        let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
            .args(["--explain", code])
            .output()
            .expect("sisuc starts");
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
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .args(["--explain", "E9999"])
        .output()
        .expect("sisuc starts");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "sisuc: unknown diagnostic code `E9999`\n"
    );
}

#[test]
fn unreadable_input_exits_1() {
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(env!("CARGO_TARGET_TMPDIR"))
        .args(["--check", "no_such_dir/unreadable_input_exits_1.sisu"])
        .output()
        .expect("sisuc starts");
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .starts_with("sisuc: cannot read no_such_dir/unreadable_input_exits_1.sisu: ")
    );
}

#[test]
fn oversized_input_is_a_tool_error() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let name = "oversized_input_is_a_tool_error.sisu";
    let file = fs::File::create(dir.join(name)).expect("creates");
    // `MAX_SOURCE` plus 1; the constant is private to the crate. The file is sparse.
    file.set_len((1 << 30) + 1).expect("sets the length");
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(["--check", name])
        .output()
        .expect("sisuc starts");
    fs::remove_file(dir.join(name)).expect("removes");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        format!("sisuc: cannot read {name}: the file is larger than 1 GiB\n")
    );
}

#[test]
fn check_reads_a_pipe() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .args(["--check", "/dev/stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("sisuc starts");
    // Dropping the handle closes the pipe, so sisuc sees the end of the source.
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(b"fn main() {\n    print(1)\n}\n")
        .expect("writes");
    let out = child.wait_with_output().expect("sisuc runs");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
}

/// Runs `sisuc --check` on a file named `name` that holds `bytes`; returns the exit code and
/// stderr.
fn check_bytes(name: &str, bytes: &[u8]) -> (Option<i32>, String) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join(name), bytes).expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(["--check", name])
        .output()
        .expect("sisuc starts");
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
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/run"))
        .args(["--emit", "ir", "loops.sisu"])
        .output()
        .expect("sisuc starts");
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
        &["--emit", "ir"],
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
        &["--emit", "ir"],
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .starts_with("warning[W0002]: `count` is never reassigned")
    );
    ir_sections(&String::from_utf8_lossy(&out.stdout));
}

/// A warning does not stop `--emit tokens` or `--emit ast`, as it does not stop `--check`.
#[test]
fn emit_with_a_redundant_semicolon_still_prints() {
    for (stage, first) in [("tokens", "1:1 Fn fn\n"), ("ast", "(fn main ")] {
        let out = run_on(
            "emit_with_a_redundant_semicolon_still_prints",
            "fn main() {\n    print(1);\n}\n",
            &["--emit", stage],
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
        &["--emit", "tir"],
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
        &["--emit", "mir"],
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
        &["--emit", "mir"],
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

/// A stage `--emit` does not know is a usage error, and the usage lists `mir`.
#[test]
fn emit_unknown_stage_lists_mir() {
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(env!("CARGO_TARGET_TMPDIR"))
        .args(["--emit", "bogus", "f.sisu"])
        .output()
        .expect("sisuc starts");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("usage: sisuc"), "{stderr}");
    assert!(
        stderr.contains("--emit tokens|ast|tir|mir <input.sisu>"),
        "{stderr}"
    );
}

#[test]
fn emit_with_an_error_prints_nothing_on_stdout() {
    let flags: [(&[&str], &str); 3] = [
        (&["--emit", "tir"], "error[E0001]: "),
        (&["--emit", "mir"], "error[E0001]: "),
        (
            &["--format", "json", "--emit", "tir"],
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
        let text = run_on("json_emit_keeps_stdout", SQUARE, &["--emit", stage]);
        let json = run_on(
            "json_emit_keeps_stdout",
            SQUARE,
            &["--format", "json", "--emit", stage],
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
    let cases: [&[&str]; 15] = [
        &["-O2", "--explain", "E0001"],
        &["-O2", "--emit", "mir", "f.sisu"],
        &["--explain", "E0001", "-O2"],
        &["-O2", "--check", "f.sisu"],
        &["-O2", "--emit", "tokens", "f.sisu"],
        &["-O2", "--emit", "ast", "f.sisu"],
        &["-O2", "--emit", "tir", "f.sisu"],
        &["-O2", "--emit", "ir-raw", "f.sisu"],
        &["f.sisu", "out", "-O2"],
        &["--emit", "ir", "-O2", "f.sisu"],
        &["--check", "-O2", "f.sisu"],
        &["-O2", "-O2", "f.sisu", "out"],
        &["--emit", "ir", "-O2"],
        &["--check", "-O2"],
        &["--emit", "ir-raw", "-O2"],
    ];
    assert_usage_errors(&cases);
}

/// `-O3` takes `-O2`'s place, and `--linker` follows an optional `--rc-checks`, for a build only.
#[test]
fn o3_and_linker_combinations_are_usage_errors() {
    let cases: [&[&str]; 12] = [
        &["-O2", "-O3", "f.sisu", "out"],
        &["-O3", "-O2", "f.sisu", "out"],
        &["-O3", "--check", "f.sisu"],
        &["-O3", "--emit", "ir-raw", "f.sisu"],
        &["--rc-checks", "-O3", "f.sisu", "out"],
        &["--linker", "lld", "--emit", "ir", "f.sisu"],
        &["--linker", "lld", "--check", "f.sisu"],
        &["--linker", "lld", "--rc-checks", "f.sisu", "out"],
        &["--linker", "lld", "--linker", "lld", "f.sisu", "out"],
        &["--linker", "ld.lld", "f.sisu", "out"],
        &["f.sisu", "out", "--linker", "lld"],
        &["--linker"],
    ];
    assert_usage_errors(&cases);
}

#[test]
fn emit_ir_raw_parses_back() {
    let out = run_on("emit_ir_raw_parses_back", FIB, &["--emit", "ir-raw"]);
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
        let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
            .current_dir(dir)
            .args(["--emit", stage, "loops.sisu"])
            .output()
            .expect("sisuc starts");
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
    let out = run_on("emit_ir_o2", SQUARE, &["-O2", "--emit", "ir"]);
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
        let out = run_on("emit_ir_o3", UNSWITCH, &[level, "--emit", "ir"]);
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
    let out = run_on("print_in_a_loop", source, &["-O2", "--emit", "ir"]);
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
    let out = run_on("inclusive_for_folds", source, &["-O2", "--emit", "ir"]);
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
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(env!("CARGO_TARGET_TMPDIR"))
        .args(["--format", "json", "--check", "no_such_dir/missing.sisu"])
        .output()
        .expect("sisuc starts");
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

/// `--format` is accepted only as the first two arguments, with `text` or `json`, and never with
/// `--explain`. A usage error stays text, also after `--format json`.
#[test]
fn format_usage_errors() {
    assert_usage_errors(&[
        &["--format", "json"],
        &["--format", "yaml", "--check", "a.sisu"],
        &["--format", "text", "--explain", "E0001"],
        // A later `--format` would otherwise fill the input slot of a valid shape.
        &["--check", "--format"],
        &["--emit", "tir", "--format"],
        &["-O2", "--emit", "ir", "--format"],
        &["--explain", "--format"],
    ]);
}

#[test]
fn json_link_failure_stays_json() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("json_link_failure.sisu"), "fn main() {}\n").expect("writes");
    // A directory `ld` cannot write to: sisuc finds a missing one before it compiles.
    let readonly = dir.join("json_link_failure_dir");
    let _ = fs::remove_dir_all(&readonly);
    fs::create_dir(&readonly).expect("creates");
    fs::set_permissions(&readonly, fs::Permissions::from_mode(0o555)).expect("sets");
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args([
            "--format",
            "json",
            "json_link_failure.sisu",
            "json_link_failure_dir/out",
        ])
        .output()
        .expect("sisuc starts");
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
    // cc's own stderr follows the status, and names the output it could not write.
    let (status, detail) = message
        .split_once('\n')
        .expect("cc's output after a newline");
    assert!(status.starts_with("cc failed: "), "{message}");
    assert!(detail.contains("json_link_failure_dir/out"), "{message}");
    assert_eq!(detail, detail.trim(), "{message}");
}

#[test]
fn json_overwrite_is_a_tool_error() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join("json_overwrite.sisu"), "fn main() {}\n").expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args([
            "--format",
            "json",
            "json_overwrite.sisu",
            "json_overwrite.sisu",
        ])
        .output()
        .expect("sisuc starts");
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
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .arg("warnings_only_build.sisu")
        .arg(&exe)
        .output()
        .expect("sisuc starts");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.ends_with(
            "\n\nsisuc: 1 warning\nsisuc: for more on a code, run `sisuc --explain <code>`: W0002\n"
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
    let plain = run_on("format_text_is_the_default", source, &["--check"]);
    let text = run_on(
        "format_text_is_the_default",
        source,
        &["--format", "text", "--check"],
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
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(["--emit", "tokens", "emit_tokens_format_spec.sisu"])
        .output()
        .expect("sisuc starts");
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
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(["--emit", "tokens", "emit_tokens_enum.sisu"])
        .output()
        .expect("sisuc starts");
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
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(["--emit", "tokens", "emit_tokens_match.sisu"])
        .output()
        .expect("sisuc starts");
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
    let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
        .current_dir(dir)
        .args(["--emit", "tokens", "emit_tokens_pipe.sisu"])
        .output()
        .expect("sisuc starts");
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
    let out = run_on("enum_layouts_follow_the_target", &source, &["--emit", "ir"]);
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

/// A `sisuc` with no runtime library beside it says so before it compiles: with no
/// temporary directory, a compile would fail at the object file first.
#[test]
fn missing_runtime_is_found_before_the_compile() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("missing_runtime");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("creates");
    let sisuc = dir.join("sisuc");
    fs::hard_link(env!("CARGO_BIN_EXE_sisuc"), &sisuc).expect("links");
    fs::write(dir.join("one.sisu"), "fn main() {\n    print(1)\n}\n").expect("writes");
    let out = Command::new(&sisuc)
        .current_dir(&dir)
        .env("TMPDIR", dir.join("absent"))
        .args(["one.sisu", "one"])
        .output()
        .expect("sisuc starts");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("sisuc: runtime library not found at "),
        "{stderr}"
    );
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
            "sisuc: output directory missing does not exist\n",
        ),
        ("outdir", "sisuc: output outdir is a directory\n"),
        (
            "regular/one",
            "sisuc: output directory regular is not a directory\n",
        ),
        (
            "regular/sub/one",
            "sisuc: cannot access output directory regular/sub: Not a directory (os error 20)\n",
        ),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_sisuc"))
            .current_dir(&dir)
            .env("TMPDIR", dir.join("absent"))
            .args(["one.sisu", output])
            .output()
            .expect("sisuc starts");
        assert_eq!(out.status.code(), Some(1), "{output}");
        assert_eq!(String::from_utf8_lossy(&out.stderr), expected);
    }
}
