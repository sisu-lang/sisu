//! The prelude: `print`, `eprint`, `panic` and `read_line`, the names no top-level item can take,
//! and `todo`.

use super::generics::infers;
use super::*;
use crate::source::PRELUDE_BASE;

/// A function `f` over `params`, whose body is `body` from line 2, and an empty `main`.
fn body(params: &str, body: &str) -> String {
    format!("fn f({params}) {{\n    {body}\n}}\nfn main() {{}}")
}

#[test]
fn panic_lowers() {
    let (program, ds) = checked("fn main() {\n    panic(\"x\")\n}");
    assert!(ds.is_empty(), "{ds:?}");
    let program = program.expect("no errors");
    assert!(program.to_string().contains("(panic \"x\")"), "{program}");
    let panic = program.functions[0]
        .body
        .value
        .as_ref()
        .expect("`panic(\"x\")` should be the body's value");
    assert_eq!(panic.ty, Type::NEVER);
}

/// The panic's location, and its count error, are the callee's, not the parentheses' around it.
#[test]
fn panic_in_parentheses() {
    let src = "fn main() {\n    (panic(\"x\"))\n}";
    let (program, ds) = checked(src);
    assert!(ds.is_empty(), "{ds:?}");
    let program = program.expect("no errors");
    let panic = program.functions[0]
        .body
        .value
        .as_ref()
        .expect("the `panic` should be the body's value");
    assert_eq!(&src[panic.span.start()..][..5], "panic", "{src}");
    codes(&body("", "(panic())"), &[(Code::ArgumentCount, (2, 6))]);
}

/// A message that is not a `string`: in each row, the body, the argument reported, and the
/// body the safe fix makes, if there is one.
#[test]
fn panic_message_must_be_string() {
    for (statements, pos, fixed) in [
        ("panic(1)", (2, 11, "1"), Some("panic(\"{1}\")")),
        ("panic(true)", (2, 11, "true"), Some("panic(\"{true}\")")),
        (
            "let b: u8 = 1\n    panic(b)",
            (3, 11, "b"),
            Some("let b: u8 = 1\n    panic(\"{b}\")"),
        ),
        (
            "let c = 'a'\n    panic(c)",
            (3, 11, "c"),
            Some("let c = 'a'\n    panic(\"{c}\")"),
        ),
        ("let xs = [1]\n    panic(xs)", (3, 11, "xs"), None),
        ("panic(1 +\n 2)", (2, 11, "1 +\n 2"), None),
    ] {
        let src = body("", statements);
        let d = diagnostic(&src, Code::Mismatch, pos, &[]);
        match fixed {
            Some(fixed) => {
                assert!(fixes(&src, &d).iter().all(|(safe, _)| *safe), "{src}");
                assert_eq!(applied(&src, &d), body("", fixed));
                fix_removes_its_code(&src, &d);
            }
            None => assert_eq!(fixes(&src, &d), [], "{src}"),
        }
    }
}

#[test]
fn panic_argument_count() {
    for statement in ["panic()", "panic(\"a\", \"b\")"] {
        codes(&body("", statement), &[(Code::ArgumentCount, (2, 5))]);
    }
}

/// `panic` never finishes: a branch, a body's tail and the right side of `??` take it.
#[test]
fn panic_is_never() {
    for src in [
        body("", "let n: i64? = None\n    let m = n ?? panic(\"x\")"),
        body("c: bool", "let n = if c { 1 } else { panic(\"x\") }"),
        "fn f() -> i64 {\n    panic(\"x\")\n}\nfn main() {}".to_string(),
    ] {
        clean(&src);
    }
    codes(
        &body("", "panic(\"x\")\n    print(1)"),
        &[(Code::UnreachableCode, (3, 5))],
    );
}

/// Anywhere else a value is used, `panic` is the "no value" error, as a `return` is.
#[test]
fn panic_is_not_a_value() {
    for (statement, col) in [
        ("let x = panic(\"x\")", 13),
        ("g(panic(\"x\"))", 7),
        ("let y = 1 + panic(\"x\")", 17),
    ] {
        let src = format!("fn g(n: i64) {{}}\n{}", body("", statement));
        codes(&src, &[(Code::NoValue, (3, col))]);
    }
}

/// A message that fails to check makes the `panic` call `Error`, as a failed argument rule
/// does a call: only the message's own error is reported, not "no value" for the call too.
#[test]
fn panic_with_a_broken_message_is_an_error() {
    for (statement, expected) in [
        (
            "let x = panic(missing)",
            &[(Code::BindingNotFound, (3, 19))][..],
        ),
        (
            "let x = panic(\"{missing}\")",
            &[(Code::BindingNotFound, (3, 21))],
        ),
        ("let x = panic(panic(\"x\"))", &[(Code::NoValue, (3, 19))]),
        ("let x = panic(1)", &[(Code::Mismatch, (3, 19))]),
        ("g(panic(1))", &[(Code::Mismatch, (3, 13))]),
        ("let y = 1 + panic(1)", &[(Code::Mismatch, (3, 23))]),
        (
            "let n: i64? = None\n    let m = n ?? panic(1)",
            &[(Code::Mismatch, (4, 24))],
        ),
    ] {
        let src = format!("fn g(n: i64) {{}}\n{}", body("", statement));
        codes(&src, expected);
    }
}

/// A `unit` right side of `??` is the "no value" error, reported once.
#[test]
fn coalesce_unit_is_not_a_value() {
    codes(
        "fn g() {}\nfn f(x: i64?) {\n    print(x ?? g())\n}\nfn main() {}",
        &[(Code::NoValue, (3, 16))],
    );
}

/// `eprint` is `print` to standard error: the same operand rules, and a node of its own.
#[test]
fn eprint_lowers() {
    let (program, ds) = checked("fn main() {\n    eprint(1)\n}");
    assert!(ds.is_empty(), "{ds:?}");
    let program = program.expect("no errors");
    assert!(program.to_string().contains("(eprint 1)"), "{program}");
}

/// `eprint`'s argument errors are `print`'s: in each row, the body, the code, and where it is
/// reported (the call for a count, the argument otherwise). An object and an array print.
#[test]
fn eprint_should_report_the_argument_errors_of_print() {
    let class = "class P {\n    x: i64\n}\n";
    for (statement, code, pos) in [
        ("eprint()", Code::PrintArgument, (5, 5)),
        ("eprint(1, 2)", Code::PrintArgument, (5, 5)),
        ("eprint(panic(\"x\"))", Code::PrintNever, (5, 12)),
    ] {
        codes(
            &format!("{class}fn main() {{\n    {statement}\n}}"),
            &[(code, pos)],
        );
    }
}

#[test]
fn eprint_should_accept_an_object_and_an_array() {
    clean("class P {\n    x: i64\n}\nfn main() {\n    eprint(P(x: 1))\n    eprint([1])\n}");
}

#[test]
fn read_line_lowers() {
    let (program, ds) = checked("fn main() {\n    let s = read_line()\n}");
    assert!(ds.is_empty(), "{ds:?}");
    let program = program.expect("no errors");
    assert!(program.to_string().contains("(read_line)"), "{program}");
    assert_eq!(
        program.functions[0].locals[0].ty,
        Type::optional(Type::STRING)
    );
}

#[test]
fn read_line_argument_count() {
    codes(&body("", "read_line(1)"), &[(Code::ArgumentCount, (2, 5))]);
}

/// A name from another language, near a prelude name or on its alias list, gets the
/// unknown-function code with the prelude name as its fix.
#[test]
fn renamed_functions() {
    for (call, fixed) in [
        ("println(1)", "print(1)"),
        ("eprintln(1)", "eprint(1)"),
        ("readLine()", "read_line()"),
        ("readln()", "read_line()"),
        ("input()", "read_line()"),
    ] {
        let src = body("", call);
        codes(&src, &[(Code::FunctionNotFound, (2, 5))]);
        assert_eq!(applied(&src, &diags(&src)[0]), body("", fixed));
    }
}

/// No top-level function, class or constant takes a prelude name.
#[test]
fn prelude_defined_twice() {
    for name in ["panic", "print", "eprint", "read_line"] {
        for (item, col) in [
            (format!("fn {name}() {{}}"), 4),
            (format!("class {name} {{}}"), 7),
            (format!("const {name} = 1"), 7),
        ] {
            let src = format!("{item}\nfn main() {{}}");
            diagnostic(&src, Code::DefinedTwice, (1, col, name), &[]);
        }
    }
}

/// A local, a parameter and a method may take a prelude name.
#[test]
fn prelude_names_free_elsewhere() {
    clean(
        "class P {\n    fn print(self) {}\n}\nfn f(panic: i64) {\n    let print = 1\n}\nfn main() {}",
    );
}

#[test]
fn todo_lowers() {
    let src =
        "fn f() -> i64 {\n    todo\n}\nfn g() -> string {\n    todo(\"x {1}\")\n}\nfn main() {}";
    codes(src, &[(Code::TodoLeft, (2, 5)), (Code::TodoLeft, (5, 5))]);
    let printed = checked(src).0.expect("only warnings").to_string();
    for part in [
        "(panic \"not yet implemented\")",
        "(panic (concat \"not yet implemented: \" ",
    ] {
        assert!(printed.contains(part), "{part} in {printed}");
    }
}

/// `todo`'s message follows `panic`'s rules, as a statement and as a value, where a broken
/// `todo` is `Error` and so not also "no value". In each row, the body, the warning's position,
/// the other diagnostic, and the body its fix makes, if it has one.
#[test]
fn todo_message_rules() {
    for (statements, warning, (code, pos), fixed) in [
        (
            "todo(1)",
            (2, 5),
            (Code::Mismatch, (2, 10)),
            Some("todo(\"{1}\")"),
        ),
        (
            "todo(\"a\", \"b\")",
            (2, 5),
            (Code::ArgumentCount, (2, 5)),
            None,
        ),
        (
            "let xs = [1]\n    todo(xs)",
            (3, 5),
            (Code::Mismatch, (3, 10)),
            None,
        ),
        (
            "let x = todo(1)",
            (2, 13),
            (Code::Mismatch, (2, 18)),
            Some("let x = todo(\"{1}\")"),
        ),
        (
            "let x = todo(\"a\", \"b\")",
            (2, 13),
            (Code::ArgumentCount, (2, 13)),
            None,
        ),
        (
            "let xs = [1]\n    let x = todo(xs)",
            (3, 13),
            (Code::Mismatch, (3, 18)),
            None,
        ),
    ] {
        let src = body("", statements);
        codes(&src, &[(Code::TodoLeft, warning), (code, pos)]);
        let d = &diags(&src)[1];
        match fixed {
            Some(fixed) => {
                assert_eq!(applied(&src, d), body("", fixed));
                fix_removes_its_code(&src, d);
            }
            None => assert_eq!(fixes(&src, d), [], "{src}"),
        }
    }
}

/// `todo` never finishes, as `panic` does: only the warning is reported.
#[test]
fn todo_is_never() {
    for (src, expected) in [
        (
            body("", "let n: i64? = None\n    print(n ?? todo)"),
            &[(Code::TodoLeft, (3, 16))][..],
        ),
        (
            body("c: bool", "let n = if c { 1 } else { todo }"),
            &[(Code::TodoLeft, (2, 31))],
        ),
        (
            body("", "todo\n    print(1)"),
            &[(Code::TodoLeft, (2, 5)), (Code::UnreachableCode, (3, 5))],
        ),
    ] {
        codes(&src, expected);
    }
}

/// Anywhere else a value is used, `todo` is the "no value" error, as `panic` is.
#[test]
fn todo_is_not_a_value() {
    for (statement, col) in [
        ("let x = todo", 13),
        ("g(todo)", 7),
        ("let y = 1 + todo", 17),
    ] {
        let src = format!("fn g(n: i64) {{}}\n{}", body("", statement));
        codes(
            &src,
            &[(Code::TodoLeft, (3, col)), (Code::NoValue, (3, col))],
        );
    }
}

/// A labelled message is reported once, and makes the `todo` `Error`, as it does a `panic`.
#[test]
fn todo_with_a_label_is_an_error() {
    codes(
        &body("", "let x = todo(m: \"x\")"),
        &[(Code::TodoLeft, (2, 13)), (Code::LabeledArgument, (2, 18))],
    );
}

/// The warning, and the panic's location, are the keyword's, not the parentheses' around it. A
/// comment between them must not split the span.
#[test]
fn todo_in_parentheses() {
    for (src, pos) in [
        ("fn main() {\n    (todo)\n}", (2, 6)),
        ("fn main() {\n    (//é\n     todo)\n}", (3, 6)),
    ] {
        diagnostic(src, Code::TodoLeft, (pos.0, pos.1, "todo"), &[]);
        let program = checked(src).0.expect("only a warning");
        let panic = program.functions[0]
            .body
            .value
            .as_ref()
            .expect("the `todo` should be the body's value");
        assert_eq!(&src[panic.span.start()..][..4], "todo", "{src}");
    }
}

/// A line break ends a bare `todo`: `(1)` on the next line is a statement, not its message.
#[test]
fn todo_ends_at_the_line() {
    codes(
        &body("", "todo\n    (1)"),
        &[
            (Code::TodoLeft, (2, 5)),
            (Code::UnreachableCode, (3, 5)),
            (Code::UnusedValue, (3, 5)),
        ],
    );
}

/// A user's item that takes a name which the prelude declares is one E0026, at the user's name.
/// The prelude's own declaration is no second error, and no diagnostic points into the prelude.
#[test]
fn user_item_should_not_take_a_name_the_prelude_declares() {
    let prelude = prelude_program(PRELUDE_SOURCE);
    for (item, expected) in [
        ("enum Result { A }", (1, 6)),
        ("fn Result() {}", (1, 4)),
        ("class Ok {}", (1, 7)),
    ] {
        let src = format!("{item}\nfn main() {{}}");
        let (_, ds) = check(&parse_ok(&src), &prelude, &src, &[]);
        let errors: Vec<_> = ds
            .iter()
            .filter(|d| d.severity() == Severity::Error)
            .collect();
        let [error] = errors[..] else {
            panic!("{src}: {ds:?}");
        };
        assert_eq!(error.code(), Code::BuiltInName, "{src}");
        assert!(error.span().end() < PRELUDE_BASE as usize, "{src}");
        assert_eq!(at(&src, error.span()), expected, "{src}");
        assert!(ds.iter().all(|d| !d.span().in_prelude()), "{src}: {ds:?}");
    }
}

/// `Ok`, `Err` and `Result.Ok` build a `Result` whose type the context fixes, in each row: the
/// program, and a part of the typed tree that shows the variant built.
#[test]
fn result_should_be_built_with_bare_names() {
    with_prelude();
    let parse = "fn parse(s: string) -> Result<i64, string> {\n    if s == \"\" { return Err(\"empty\") }\n    Ok(s.len())\n}\nfn main() {}";
    let r = "let r: Result<i64, string> =";
    for (src, part) in [
        (body("", &format!("{r} Ok(1)")), "Result.Ok 1"),
        (body("", &format!("{r} Err(\"bad\")")), "Result.Err \"bad\""),
        (parse.to_string(), "Result.Ok"),
        (body("", &format!("{r} Result.Ok(1)")), "Result.Ok 1"),
        (
            body("", "let r = Result<i64, string>.Err(\"x\")"),
            "Result.Err \"x\"",
        ),
    ] {
        let printed = lowered(&src);
        assert!(printed.contains(part), "{part} in {printed}");
    }
}

/// `Ok` and `Err` need their context whatever their argument is, as each variant names one of
/// the two parameters of `Result`: inside a generic call, or inside another `Result`.
#[test]
fn result_should_wait_for_its_context_whatever_its_argument() {
    with_prelude();
    let r = "Result<i64, string>";
    infers(&[
        (
            &format!("fn g() {{\n    let r: {r} = id(Ok(1))\n}}"),
            "id",
            &[r, r],
        ),
        (
            &format!("fn g() {{\n    let r: {r} = id(Err(\"x\"))\n}}"),
            "id",
            &[r, r],
        ),
    ]);
    let nested = "let r: Result<Result<i64, string>, string> = Ok(Err(\"x\"))";
    contains(
        &body("", nested),
        &["(variant Result.Ok (variant Result.Err \"x\"))"],
    );
}

/// With no annotation, `id(Ok(1))` has no context to fix `E`, and `id` cannot infer `T`.
#[test]
fn result_should_not_be_inferred_from_its_argument_alone() {
    with_prelude();
    let src = format!(
        "{}\nfn id<T>(x: T) -> T {{ x }}",
        body("", "let r = id(Ok(1))")
    );
    codes(&src, &[(Code::CannotInfer, (2, 13))]);
}

/// A `match` takes `Ok` and `Err` as the variants of the `Result` it matches.
#[test]
fn result_should_be_matched_with_bare_names() {
    with_prelude();
    clean(&body(
        "r: Result<i64, string>",
        "let n = match r {\n        Ok(n) => n\n        Err(e) => 0\n    }",
    ));
}

/// What a `Result` cannot decide: in each row, the program, the code and where it is reported.
#[test]
fn result_should_report_what_it_cannot_decide() {
    with_prelude();
    for (src, expected) in [
        (
            body("", "let r = Ok(1)"),
            &[(Code::CannotInfer, (2, 13))][..],
        ),
        (
            body("", "let r = Err(\"x\")"),
            &[(Code::CannotInfer, (2, 13))],
        ),
        (
            body(
                "r: Result<i64, string>",
                "let n = match r {\n        Ok(n) => n\n    }",
            ),
            &[(Code::NonExhaustive, (2, 13))],
        ),
        (body("", "let r = Ok"), &[(Code::VariantForm, (2, 13))]),
        (
            body("", "let r: Result<i64, string> = Ok(1, 2)"),
            &[(Code::ArgumentCount, (2, 34))],
        ),
    ] {
        codes(&src, expected);
    }
}

/// `Ok` and `Err` name the variants of a `Result` that declares them: with no prelude, or with
/// a user's `Result` that has none (which E0026 reports), a call of either names nothing.
#[test]
fn ok_should_name_nothing_when_no_result_declares_it() {
    let call = body("", "let r = Ok(1)");
    codes(&call, &[(Code::FunctionNotFound, (2, 13))]);
    with_prelude();
    let src = format!("enum Result {{\n    A\n}}\n{call}");
    codes(
        &src,
        &[
            (Code::BuiltInName, (1, 6)),
            (Code::FunctionNotFound, (5, 13)),
        ],
    );
}

/// `dbg(e)` is a node that holds the argument's source text as written, and has its type.
#[test]
fn dbg_should_lower_to_a_node_with_the_source_text() {
    contains(
        "fn f(x: i64) -> i64 {\n    x\n}\nfn main() {\n    let y = dbg(f(1 +   2)) + 1\n    print(y)\n}",
        &["(dbg \"f(1 +   2)\" (call f (+ 1 2)))"],
    );
}

/// `dbg(e)` has the type of `e`, checked against the expected type as `e` alone is, and gives no
/// warning, an optional included.
#[test]
fn dbg_should_check_clean_with_the_type_of_its_argument() {
    for (row, statements) in [
        ("a sum", "let y = dbg(f(1)) + 1\n    print(y)"),
        ("an expected u8", "let b: u8 = dbg(200)\n    print(b)"),
        (
            "an expected array",
            "let xs: [u8] = dbg([1, 2])\n    print(xs)",
        ),
        (
            "an optional",
            "let name: string? = None\n    let _ = dbg(name)",
        ),
        ("a local named dbg", "let dbg = 1\n    print(dbg)"),
    ] {
        let src = format!("fn f(x: i64) -> i64 {{\n    x\n}}\nfn main() {{\n    {statements}\n}}");
        clean_row(row, &src);
    }
}

/// `dbg` reports an argument that has no value, never finishes or is not one as `print` does.
#[test]
fn dbg_should_report_the_argument_errors_of_print() {
    for (statement, code, pos) in [
        ("dbg(greet())", Code::PrintArgument, (3, 5)),
        ("dbg(panic(\"x\"))", Code::PrintNever, (3, 9)),
        ("dbg()", Code::PrintArgument, (3, 5)),
        ("dbg(1, 2)", Code::PrintArgument, (3, 5)),
    ] {
        let src = format!("fn greet() {{}}\nfn main() {{\n    {statement}\n}}");
        codes_row(statement, &src, &[(code, pos)]);
    }
}

/// No top-level function, class or constant takes the name `dbg`.
#[test]
fn dbg_should_be_a_prelude_name() {
    for (item, col) in [
        ("fn dbg() {}", 4),
        ("class dbg {}", 7),
        ("const dbg = 1", 7),
    ] {
        let src = format!("{item}\nfn main() {{}}");
        diagnostic(&src, Code::DefinedTwice, (1, col, "dbg"), &[]);
    }
}

/// An argument that fails to check is reported once, and `dbg` adds no error of its own.
#[test]
fn dbg_should_report_a_broken_argument_once() {
    for (statement, code, pos) in [
        ("dbg(missing)", Code::BindingNotFound, (2, 9)),
        ("dbg(1 + \"a\")", Code::Mismatch, (2, 9)),
    ] {
        codes_row(statement, &body("", statement), &[(code, pos)]);
    }
}
