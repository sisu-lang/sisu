//! The prelude: `print`, `eprint`, `panic` and `read_line`, the names no top-level item can take,
//! and `todo`.

use super::generics::infers;
use super::*;
use crate::front_end::{self, LANG_SOURCE};
use crate::lexer::{self, TokenKind};
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

/// A message that fails to check leaves the `panic` call its type `never`, as a failed argument
/// rule leaves a call its type: a `let` that holds the call reports "no value" too.
#[test]
fn a_panic_with_a_broken_message_should_keep_its_type() {
    for (statement, expected) in [
        (
            "let x = panic(missing)",
            &[(Code::NoValue, (3, 13)), (Code::BindingNotFound, (3, 19))][..],
        ),
        (
            "let x = panic(\"{missing}\")",
            &[(Code::NoValue, (3, 13)), (Code::BindingNotFound, (3, 21))],
        ),
        (
            "let x = panic(panic(\"x\"))",
            &[(Code::NoValue, (3, 13)), (Code::NoValue, (3, 19))],
        ),
        (
            "let x = panic(1)",
            &[(Code::NoValue, (3, 13)), (Code::Mismatch, (3, 19))],
        ),
        (
            "g(panic(1))",
            &[(Code::NoValue, (3, 7)), (Code::Mismatch, (3, 13))],
        ),
        (
            "let y = 1 + panic(1)",
            &[(Code::NoValue, (3, 17)), (Code::Mismatch, (3, 23))],
        ),
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
    let class = "class P {\n    x: i64\n}\nfn nothing() {}\n";
    for (statement, code, pos) in [
        ("eprint()", Code::PrintArgument, (6, 5)),
        ("eprint(1, 2)", Code::PrintArgument, (6, 5)),
        ("eprint(nothing())", Code::PrintArgument, (6, 12)),
        ("eprint(panic(\"x\"))", Code::PrintNever, (6, 12)),
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

/// A prelude function that `prelude.sisu` writes, such as `exchange`, takes a name from every
/// top-level item in the same way as `print`: the item gets "is a built-in function". A local, a
/// parameter and a method may take the name still.
#[test]
fn exchange_should_be_a_prelude_name_for_top_level_items() {
    with_prelude();
    for (item, col) in [
        ("fn exchange() {}", 4),
        ("class exchange {}", 7),
        ("const exchange = 1", 7),
    ] {
        let src = format!("{item}\nfn main() {{}}");
        diagnostic_row(item, &src, Code::DefinedTwice, (1, col, "exchange"), &[]);
    }
    clean(
        "class P {\n    fn exchange(self) {}\n}\nfn f(exchange: i64) {\n    let exchange = 1\n}\nfn main() {}",
    );
}

/// A top-level item that a prelude function's name rejects poisons the name: its uses read neither
/// the built-in nor the item, so they add no error beside the rejection. In each row: the program
/// and the place of the rejection, the only error.
#[test]
fn an_item_with_a_prelude_function_name_should_poison_its_uses() {
    with_prelude();
    for (row, src, at) in [
        (
            "function print",
            "fn print(x: i64, y: i64) -> i64 {\n    x\n}\n\nfn main() {\n    print(1, 2)\n}",
            (1, 4),
        ),
        (
            "class print",
            "class print {\n    x: i64\n}\n\nfn main() {\n    print(x: 1)\n}",
            (1, 7),
        ),
        (
            "class exchange",
            "class exchange {\n    x: i64\n}\n\nfn main() {\n    let e = exchange(x: 1)\n    print(e.x)\n}",
            (1, 7),
        ),
        (
            "constant print",
            "const print = 1\n\nfn main() {\n    print(print)\n}",
            (1, 7),
        ),
        (
            "function exchange",
            "fn exchange(a: i64) -> i64 {\n    a\n}\n\nfn main() {\n    print(exchange(1))\n}",
            (1, 4),
        ),
        (
            "class print as a type",
            "class print {\n    x: i64\n}\n\nfn f(p: print) {}\nfn main() {}",
            (1, 7),
        ),
        (
            "enum dbg as a type",
            "enum dbg {\n    A\n}\n\nfn f(d: dbg) {}\nfn main() {}",
            (1, 6),
        ),
        (
            "alias read_line as a type",
            "type read_line = i64\n\nfn main() {\n    let x: read_line = 1\n}",
            (1, 6),
        ),
        (
            "interface panic",
            "interface panic {\n    fn f(self)\n}\n\nfn main() {\n    panic(1, 2)\n}",
            (1, 11),
        ),
        (
            "class dbg",
            "class dbg {}\nfn main() {\n    let v = dbg([])\n}",
            (1, 7),
        ),
    ] {
        codes_row(row, src, &[(Code::DefinedTwice, at)]);
    }
}

/// `codes_row` for a program that may hold a broken item, which `checked` does not parse.
fn front_end_row(row: &str, src: &str, expected: &[Wanted]) {
    let ds = front_end::front_end(src).1;
    let found: Vec<_> = ds.iter().map(|d| (d.code(), at(src, d.span()))).collect();
    assert_eq!(found, expected, "row `{row}`");
}

/// A rejected enum: `E0034` at 1:6.
const REJECTED_ENUM: &str = "enum print {\n    A\n}\n\n";
/// A rejected constant: `E0034` at 1:7.
const REJECTED_CONST: &str = "const print = 1\n\n";
/// A broken enum: `E0004` at 2:1.
const BROKEN_ENUM: &str = "enum Foo\n{ A }\n\n";
/// A broken interface: `E0004` at 2:1.
const BROKEN_INTERFACE: &str = "interface Foo\n{}\n\n";
/// A broken function: `E0004` at 2:1.
const BROKEN_FN: &str = "fn foo()\n{}\n\n";
/// The only error of a rejected enum.
const E_ENUM: &[Wanted] = &[(Code::DefinedTwice, (1, 6))];
/// The only error of a rejected constant.
const E_CONST: &[Wanted] = &[(Code::DefinedTwice, (1, 7))];
/// The only error of a broken item.
const E_BROKEN: &[Wanted] = &[(Code::BraceOnNewLine, (2, 1))];

/// The arm `{pattern} => {}`, or `pattern` itself when it holds the `=>` already.
fn arm(pattern: &str) -> String {
    if pattern.contains("=>") {
        pattern.to_string()
    } else {
        format!("{pattern} => {{}}")
    }
}

/// A `main` that matches `1` with the arm `{pattern} => {}` and then `_ => {}`.
fn match_one(pattern: &str) -> String {
    format!(
        "fn main() {{\n    match 1 {{\n        {}\n        _ => {{}}\n    }}\n}}",
        arm(pattern)
    )
}

/// A function over a `Z` that matches it with the arm `{pattern} => {}` and then `_ => {}`.
fn match_enum(pattern: &str) -> String {
    format!(
        "enum Z {{ Q }}\nfn f(z: Z) {{\n    match z {{\n        {}\n        _ => {{}}\n    }}\n}}\nfn main() {{}}",
        arm(pattern)
    )
}

/// A function over an `i64?` that matches it with the arm `{pattern} => {}` and then `_ => {}`.
fn match_option(pattern: &str) -> String {
    format!(
        "fn main() {{\n    let o: i64? = None\n    match o {{\n        {}\n        _ => {{}}\n    }}\n}}",
        arm(pattern)
    )
}

/// A `main` that declares `let o: {ty} = None` and then runs `statement`.
fn with_o(ty: &str, statement: &str) -> String {
    format!("fn main() {{\n    let o: {ty} = None\n    {statement}\n}}")
}

/// A generic call of a poisoned name reads neither the built-in nor the item: it gives no
/// `E0137` beside the rejection. A local of that name is not poisoned and keeps its error.
#[test]
fn a_generic_call_of_a_poisoned_name_should_give_no_second_error() {
    let call = "fn main() {\n    print<i64>(1, 2)\n}";
    front_end_row(
        "rejected enum, call",
        &[REJECTED_ENUM, call].concat(),
        E_ENUM,
    );
    front_end_row(
        "rejected constant, call",
        &[REJECTED_CONST, call].concat(),
        E_CONST,
    );
    front_end_row(
        "rejected constant, path",
        &[
            REJECTED_CONST,
            "fn main() {\n    let _a = print<i64>.x(1)\n}",
        ]
        .concat(),
        E_CONST,
    );
    front_end_row(
        "a local keeps its error",
        &[
            REJECTED_CONST,
            "fn main() {\n    let print = 1\n    print(print<i64>(2))\n}",
        ]
        .concat(),
        &[(Code::DefinedTwice, (1, 7)), (Code::TypeArguments, (5, 11))],
    );
    front_end_row(
        "no poison stays",
        "fn main() {\n    print<i64>(1)\n}",
        &[(Code::TypeArguments, (2, 10))],
    );
    front_end_row(
        "rejected function",
        &["fn print() {}\n\n", call].concat(),
        &[(Code::DefinedTwice, (1, 4))],
    );
    front_end_row(
        "a parameter keeps its error",
        &[
            REJECTED_CONST,
            "fn f(print: i64) {\n    let _a = print<i64>(2)\n}\nfn main() {}",
        ]
        .concat(),
        &[(Code::DefinedTwice, (1, 7)), (Code::TypeArguments, (4, 14))],
    );
    front_end_row(
        "a local keeps its error, path",
        &[
            REJECTED_CONST,
            "fn main() {\n    let print = 1\n    let _a = print<i64>.x(2)\n}",
        ]
        .concat(),
        &[(Code::DefinedTwice, (1, 7)), (Code::TypeArguments, (5, 14))],
    );
    front_end_row(
        "intact enum named i64, generic call",
        "enum i64 { A }\n\nfn main() {\n    i64<i64>(1)\n}",
        &[
            (Code::BuiltInName, (1, 6)),
            (Code::TypeArguments, (4, 8)),
            (Code::AlreadyConverted, (4, 14)),
        ],
    );
    front_end_row(
        "intact enum named Result, generic call",
        "enum Result { A }\n\nfn main() {\n    Ok<i64>(1)\n}",
        &[(Code::BuiltInName, (1, 6)), (Code::TypeArguments, (4, 7))],
    );
}

/// A local that takes the name of a poisoned enum gives no `E0116`: the enum does not exist. An
/// intact enum, and a duplicate one, still clash.
#[test]
fn a_local_with_a_poisoned_enum_name_should_give_no_second_error() {
    let rejected = |body: &str| [REJECTED_ENUM, body].concat();
    let broken = |body: &str| [BROKEN_ENUM, body].concat();
    front_end_row(
        "rejected enum, let",
        &rejected("fn main() {\n    let print = 1\n}"),
        E_ENUM,
    );
    front_end_row(
        "rejected enum, parameter",
        &rejected("fn f(print: i64) {}\nfn main() {}"),
        E_ENUM,
    );
    front_end_row(
        "rejected enum, for",
        &rejected("fn main() {\n    for print in [1] {}\n}"),
        E_ENUM,
    );
    front_end_row(
        "rejected enum, if let",
        &rejected(&with_o("i64?", "if let Some(print) = o {}")),
        E_ENUM,
    );
    front_end_row(
        "broken enum, let",
        &broken("fn main() {\n    let Foo = 1\n}"),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, parameter",
        &broken("fn f(Foo: i64) {}\nfn main() {}"),
        E_BROKEN,
    );
    front_end_row(
        "intact enum stays",
        "enum Foo { A }\n\nfn main() {\n    let Foo = 1\n}",
        &[(Code::EnumNamedLocal, (4, 9))],
    );
    front_end_row(
        "a local of a rejected enum's name keeps its type",
        &rejected("fn main() {\n    let print = 1\n    let _a: string = print\n}"),
        &[(Code::DefinedTwice, (1, 6)), (Code::Mismatch, (7, 22))],
    );
    front_end_row(
        "a local of a broken enum's name keeps its type",
        &broken("fn main() {\n    let Foo = 1\n    let _a: string = Foo\n}"),
        &[(Code::BraceOnNewLine, (2, 1)), (Code::Mismatch, (6, 22))],
    );
    front_end_row(
        "duplicate enum, let",
        "enum Foo { A }\nconst Foo = 1\n\nfn main() {\n    let Foo = 1\n}",
        &[(Code::DefinedTwice, (2, 7)), (Code::EnumNamedLocal, (5, 9))],
    );
    front_end_row(
        "broken enum, if let",
        &broken(&with_o("i64?", "if let Foo = o {}")),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, let else",
        &broken(&with_o("i64?", "let Foo = o else { return }")),
        E_BROKEN,
    );
    front_end_row(
        "rejected enum, if let bare",
        &rejected(&with_o("i64?", "if let print = o {}")),
        E_ENUM,
    );
    front_end_row(
        "rejected enum, let else",
        &rejected(&with_o("i64?", "let print = o else { return }")),
        E_ENUM,
    );
    front_end_row(
        "rejected enum, let else some",
        &rejected(&with_o("i64?", "let Some(print) = o else { return }")),
        E_ENUM,
    );
    front_end_row(
        "rejected enum, while let",
        &rejected(&with_o("i64?", "while let Some(print) = o {}")),
        E_ENUM,
    );
}

/// A bare or call-form name that is poisoned in a pattern gives no `E0125`, `E0126`, `E0118`,
/// `E0032`, `E0011` or `E0127` beside the first error. A real variant of that name, a
/// constant that is not poisoned and a name in a range keep their meaning.
#[test]
#[expect(clippy::too_many_lines, reason = "one row for each case of the table")]
fn a_poisoned_name_in_a_pattern_should_give_no_second_error() {
    let w_call = |arm: &str, rest: &str| {
        format!(
            "enum W {{\n    Foo(i64)\n    Bar\n}}\nfn f(w: W) {{\n    match w {{\n        {arm}\n        {rest}\n    }}\n}}\nfn main() {{}}"
        )
    };
    let broken = |body: &str| [BROKEN_ENUM, body].concat();
    let rejected_const = |body: &str| [REJECTED_CONST, body].concat();
    let rejected_enum = |body: &str| [REJECTED_ENUM, body].concat();

    // Poisoned names that give only the first error.
    front_end_row("broken enum, bare", &broken(&match_one("Foo")), E_BROKEN);
    front_end_row(
        "broken interface, bare",
        &[BROKEN_INTERFACE, &match_one("Foo")].concat(),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, enum position",
        &broken(&match_enum("Foo")),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, sub-pattern",
        &broken(&match_option("Some(Foo)")),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, alternative",
        &broken(&match_one("Foo | 2")),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, if let",
        &broken(&with_o("i64?", "if let Some(Foo) = o {}")),
        E_BROKEN,
    );
    front_end_row(
        "rejected constant, bare",
        &rejected_const(&match_one("print")),
        E_CONST,
    );
    front_end_row(
        "rejected constant, enum position",
        &rejected_const(&match_enum("print")),
        E_CONST,
    );
    front_end_row(
        "rejected constant, sub-pattern",
        &rejected_const(&match_option("Some(print)")),
        E_CONST,
    );
    front_end_row(
        "rejected constant, alternative",
        &rejected_const(&match_one("print | 2")),
        E_CONST,
    );
    front_end_row(
        "rejected constant, range bound",
        &rejected_const(&match_one("0..print")),
        E_CONST,
    );
    front_end_row(
        "rejected enum, range bound",
        &rejected_enum(&match_one("0..print")),
        E_ENUM,
    );
    front_end_row(
        "broken enum, range bound",
        &broken(&match_one("0..Foo")),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, call form",
        &broken(
            "fn main() {\n    match 1 {\n        Foo(x) => print(x)\n        _ => {}\n    }\n}",
        ),
        E_BROKEN,
    );
    front_end_row(
        "rejected constant, call form",
        &rejected_const(
            "fn main() {\n    match 1 {\n        print(x) => {}\n        _ => {}\n    }\n}",
        ),
        E_CONST,
    );
    front_end_row(
        "broken enum, call form at an enum",
        &broken(&match_enum("Foo(x)")),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, call form in a sub-pattern",
        &broken(&match_option("Some(Foo(x))")),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, while let",
        &broken(&with_o("i64?", "while let Some(Foo) = o {}")),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, let else some",
        &broken(&with_o("i64?", "let Some(Foo) = o else { return }")),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, let else call form",
        &broken(&with_o(
            "i64?",
            "let Foo(x) = o else { return }\n    print(x)",
        )),
        E_BROKEN,
    );
    front_end_row(
        "rejected constant, let else some",
        &rejected_const(&with_o("i64?", "let Some(print) = o else { return }")),
        E_CONST,
    );
    front_end_row(
        "broken enum, if let call form",
        &broken(&with_o("i64?", "if let Foo(x) = o { print(x) }")),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, if let range",
        &broken(&with_o("i64?", "if let 0..Foo = o {}")),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, let else range",
        &broken(&with_o("i64?", "let 0..Foo = o else { return }")),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, match on o, bare",
        &broken(&match_option("Foo")),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, match on o, call form",
        &broken(&match_option("Foo(x) => print(x)")),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, match on o, range",
        &broken(&match_option("0..Foo")),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, range of poisoned names",
        &broken(&match_one("Foo..Foo")),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, inclusive range",
        &broken(&match_one("0..=Foo")),
        E_BROKEN,
    );
    front_end_row(
        "broken enum, range at an enum",
        &broken(&match_enum("0..Foo")),
        E_BROKEN,
    );
    front_end_row(
        "broken function, range bound",
        &[BROKEN_FN, &match_one("0..foo")].concat(),
        E_BROKEN,
    );
    front_end_row(
        "broken function, call form",
        &[
            BROKEN_FN,
            "fn main() {\n    match 1 {\n        foo(x) => print(x)\n        _ => {}\n    }\n}",
        ]
        .concat(),
        E_BROKEN,
    );

    // Positions that take only names.
    let names_only = |ty: &str, arms: &str| {
        broken(&format!(
            "fn main() {{\n    let o: {ty} = None\n    match o {{\n        {arms}\n        _ => {{}}\n    }}\n}}"
        ))
    };
    front_end_row(
        "names only, call form",
        &names_only("f64?", "Foo(x) => print(x)"),
        E_BROKEN,
    );
    front_end_row(
        "names only, call form under Some",
        &names_only("f64?", "Some(Foo(x)) => print(x)"),
        E_BROKEN,
    );
    front_end_row(
        "names only, call form at an array",
        &names_only("[i64]?", "Foo(x) => print(x)"),
        E_BROKEN,
    );
    front_end_row(
        "names only, let else",
        &broken(&with_o(
            "f64?",
            "let Foo(x) = o else { return }\n    print(x)",
        )),
        E_BROKEN,
    );
    front_end_row(
        "names only, in a payload",
        &broken(
            "enum V { P(f64) }\nfn f(v: V) {\n    match v {\n        P(Foo(x)) => print(x)\n    }\n}\nfn main() {}",
        ),
        E_BROKEN,
    );
    front_end_row(
        "names only, rejected constant",
        &rejected_const(&with_o(
            "f64?",
            "match o {\n        print(x) => {}\n        _ => {}\n    }",
        )),
        E_CONST,
    );

    // What must stay.
    let real = |arm: &str, rest: &str| broken(&w_call(arm, rest));
    front_end_row(
        "a real variant, call form",
        &real("Foo(x) => print(x)", "Bar => {}"),
        E_BROKEN,
    );
    front_end_row(
        "a real variant, x has its type",
        &real("Foo(x) => { let _s: string = x }", "Bar => {}"),
        &[(Code::BraceOnNewLine, (2, 1)), (Code::Mismatch, (10, 38))],
    );
    front_end_row(
        "a real variant, wrong payload",
        &real("Foo(x, y) => print(x)", "Bar => {}"),
        &[
            (Code::BraceOnNewLine, (2, 1)),
            (Code::SubpatternCount, (10, 9)),
        ],
    );
    front_end_row(
        "a real variant, bare, wrong form",
        &real("Foo => {}", "Bar => {}"),
        &[(Code::BraceOnNewLine, (2, 1)), (Code::VariantForm, (10, 9))],
    );
    let optional = |arm: &str| {
        broken(&format!(
            "enum W {{\n    Foo(i64)\n}}\nfn f(w: W?) {{\n    match w {{\n        {arm}\n        _ => {{}}\n    }}\n}}\nfn main() {{}}"
        ))
    };
    front_end_row(
        "a real variant under an optional, call form",
        &optional("Foo(x) => print(x)"),
        E_BROKEN,
    );
    front_end_row(
        "a real variant under an optional, x has its type",
        &optional("Foo(x) => { let _s: string = x }"),
        &[(Code::BraceOnNewLine, (2, 1)), (Code::Mismatch, (9, 38))],
    );
    front_end_row(
        "a real variant, bare, coverage",
        &broken(
            "enum W {\n    Foo\n    Bar\n}\nfn f(w: W) {\n    match w {\n        Foo => {}\n    }\n}\nfn main() {}",
        ),
        &[
            (Code::BraceOnNewLine, (2, 1)),
            (Code::NonExhaustive, (9, 5)),
        ],
    );
    front_end_row(
        "a real variant, bare under an optional, coverage",
        &broken(
            "enum W {\n    Foo\n    Bar\n}\nfn f(w: W?) {\n    match w {\n        Foo => {}\n        None => {}\n    }\n}\nfn main() {}",
        ),
        &[
            (Code::BraceOnNewLine, (2, 1)),
            (Code::NonExhaustive, (9, 5)),
        ],
    );
    front_end_row(
        "range, first bound poisoned, second a name",
        &broken(&match_one("Foo..k")),
        &[(Code::BraceOnNewLine, (2, 1)), (Code::Expected, (6, 14))],
    );
    front_end_row(
        "range, second bound poisoned, first a name",
        &broken(&match_one("k..Foo")),
        &[(Code::BraceOnNewLine, (2, 1)), (Code::Expected, (6, 9))],
    );
    front_end_row(
        "range, a constant keeps its error",
        &[BROKEN_ENUM, "const K = 1\n\n", &match_one("Foo..K")].concat(),
        &[
            (Code::BraceOnNewLine, (2, 1)),
            (Code::ConstantInPattern, (8, 14)),
        ],
    );
    front_end_row(
        "range at a names-only position keeps E0127",
        &broken(
            "fn main() {\n    let o: f64? = None\n    match o {\n        0..Foo => {}\n        _ => {}\n    }\n}",
        ),
        &[
            (Code::BraceOnNewLine, (2, 1)),
            (Code::OnlyNamePosition, (7, 9)),
        ],
    );
    front_end_row(
        "a name in one alternative keeps E0136",
        &broken(&match_one("Foo(x) | 2")),
        &[
            (Code::BraceOnNewLine, (2, 1)),
            (Code::AlternativeBindings, (6, 18)),
        ],
    );
    front_end_row(
        "rejected enum, binding, _ is unreached",
        &rejected_enum(&match_one("print")),
        &[(Code::DefinedTwice, (1, 6)), (Code::UnreachableArm, (8, 9))],
    );
    front_end_row(
        "rejected enum, binding at an enum",
        &rejected_enum(&match_enum("print")),
        &[(Code::DefinedTwice, (1, 6)), (Code::UnreachableArm, (9, 9))],
    );
    front_end_row(
        "rejected enum, binding in a sub-pattern",
        &rejected_enum(&match_option("Some(print)")),
        E_ENUM,
    );
    front_end_row(
        "rejected enum, binding in an alternative",
        &rejected_enum(&match_one("print | 2")),
        &[
            (Code::DefinedTwice, (1, 6)),
            (Code::AlternativeBindings, (7, 17)),
        ],
    );
    front_end_row(
        "duplicate enum, bare",
        &["enum Foo { A }\nconst Foo = 1\n\n", &match_one("Foo")].concat(),
        &[(Code::DefinedTwice, (2, 7)), (Code::NotVariantHere, (6, 9))],
    );
    front_end_row(
        "duplicate constant, bare",
        &["const foo = 1\nconst foo = 2\n\n", &match_one("foo")].concat(),
        &[
            (Code::DefinedTwice, (2, 7)),
            (Code::ConstantInPattern, (6, 9)),
        ],
    );
    front_end_row(
        "intact enum named Result, call form",
        "enum Result { A }\n\nfn main() {\n    match 1 {\n        Ok(x) => print(x)\n        _ => {}\n    }\n}",
        &[(Code::BuiltInName, (1, 6)), (Code::Mismatch, (5, 9))],
    );
    front_end_row(
        "lower-case binding, alternative",
        &[BROKEN_FN, &match_one("foo | 2")].concat(),
        &[
            (Code::BraceOnNewLine, (2, 1)),
            (Code::AlternativeBindings, (6, 15)),
        ],
    );
    front_end_row(
        "lower-case binding, bare",
        &[BROKEN_FN, &match_one("foo")].concat(),
        &[
            (Code::BraceOnNewLine, (2, 1)),
            (Code::UnreachableArm, (7, 9)),
        ],
    );
    front_end_row(
        "no poison, upper-case",
        &match_one("Foo"),
        &[(Code::NotVariantHere, (3, 9))],
    );
    front_end_row(
        "no poison, constant",
        &["const K = 1\n\n", &match_one("K")].concat(),
        &[(Code::ConstantInPattern, (5, 9))],
    );
    front_end_row(
        "no poison, range bound on a constant",
        &["const K = 1\n\n", &match_one("0..K")].concat(),
        &[(Code::ConstantInPattern, (5, 12))],
    );
    front_end_row(
        "no poison, range bound on a name",
        &match_one("0..k"),
        &[(Code::Expected, (3, 12))],
    );
    front_end_row(
        "no poison, call form",
        &match_one("Foo(x)"),
        &[(Code::Mismatch, (3, 9))],
    );
    front_end_row(
        "a qualified name is no poisoned name, at an enum",
        &broken(
            "enum Z { Q }\nfn f(z: Z) {\n    match z {\n        Z.Foo => {}\n        _ => {}\n    }\n}\nfn main() {}",
        ),
        &[
            (Code::BraceOnNewLine, (2, 1)),
            (Code::UnknownVariant, (7, 11)),
        ],
    );
    front_end_row(
        "a qualified name is no poisoned name, at a names-only position",
        &broken(
            "enum Z { Q }\nfn main() {\n    let o: f64? = None\n    match o {\n        Z.Foo(x) => print(x)\n        _ => {}\n    }\n}",
        ),
        &[
            (Code::BraceOnNewLine, (2, 1)),
            (Code::OnlyNamePosition, (8, 9)),
        ],
    );
}

/// A read or a write of a poisoned name reports no second error. A true error beside it stays.
#[test]
#[expect(clippy::too_many_lines, reason = "one row for each case of the table")]
fn a_read_or_write_of_a_hidden_name_should_give_no_second_error() {
    // A function with the parameter `{name}: string`, the extra parameters `extra` and `body`.
    let in_f = |base: &str, name: &str, extra: &str, body: &str| {
        format!("{base}fn f({name}: string{extra}) {{\n{body}\n}}\nfn main() {{}}")
    };
    let in_main = |base: &str, body: &str| format!("{base}fn main() {{\n{body}\n}}");
    // `match 1` with the arm `{arm}` and then `_ => {}`.
    let m = |arm: &str| format!("    match 1 {{\n        {arm}\n        _ => {{}}\n    }}");
    let m_on = |subject: &str, arm: &str| {
        format!("    match {subject} {{\n        {arm}\n        _ => {{}}\n    }}")
    };
    let o = ", o: i64?";
    let (e, c) = (BROKEN_ENUM, REJECTED_CONST);

    // The outer local is hidden.
    for (row, src, expected) in [
        (
            "upper-case, bare",
            in_f(e, "Foo", "", &m("Foo => { let _a: i64 = Foo }")),
            E_BROKEN,
        ),
        (
            "constant, bare",
            in_f(c, "print", "", &m("print => { let _a: i64 = print }")),
            E_CONST,
        ),
        (
            "constant, alternative",
            in_f(c, "print", "", &m("print | 2 => { let _a: i64 = print }")),
            E_CONST,
        ),
        (
            "upper-case, twice in an alternative",
            in_f(e, "Foo", "", &m("Foo | Foo => { let _a: i64 = Foo }")),
            E_BROKEN,
        ),
        (
            "guard",
            in_f(e, "Foo", "", &m("Foo if Foo == 1 => {}")),
            E_BROKEN,
        ),
        (
            "if let",
            in_f(
                e,
                "Foo",
                o,
                "    if let Some(Foo) = o { let _a: i64 = Foo }",
            ),
            E_BROKEN,
        ),
        (
            "write to an outer parameter",
            in_f(e, "Foo", "", &m("Foo => { Foo = \"a\" }")),
            E_BROKEN,
        ),
        (
            "let else",
            in_f(
                e,
                "Foo",
                o,
                "    let Some(Foo) = o else { return }\n    let _a: i64 = Foo",
            ),
            E_BROKEN,
        ),
        (
            "while let",
            in_f(
                e,
                "Foo",
                o,
                "    while let Some(Foo) = o { let _a: i64 = Foo }",
            ),
            E_BROKEN,
        ),
    ] {
        front_end_row(row, &src, expected);
    }

    // The outer local is hidden, and the first error of the pattern stays.
    for (row, src, expected) in [
        (
            "unknown subject",
            in_f(e, "Foo", "", &m_on("zz", "Foo => { let _a: i64 = Foo }")),
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::BindingNotFound, (5, 11)),
            ][..],
        ),
        (
            "if let on a value that is not optional",
            in_f(e, "Foo", "", "    if let Foo = 1 { let _a: i64 = Foo }"),
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::UnwrapNeedsOptional, (5, 18)),
            ][..],
        ),
        (
            "f64 subject",
            in_f(e, "Foo", "", &m_on("1.5", "Foo => { let _a: i64 = Foo }")),
            &[(Code::BraceOnNewLine, (2, 1)), (Code::CannotMatch, (5, 11))][..],
        ),
        (
            "Some with two sub-patterns",
            in_f(
                e,
                "Foo",
                o,
                &m_on("o", "Some(Foo, 1) => { let _a: i64 = Foo }"),
            ),
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::SubpatternCount, (6, 9)),
            ][..],
        ),
        (
            "unknown variant with a sub-pattern",
            in_f(e, "Foo", "", &m("Bar(Foo) => { let _a: i64 = Foo }")),
            &[(Code::BraceOnNewLine, (2, 1)), (Code::Mismatch, (6, 9))][..],
        ),
        (
            "write, unknown subject",
            in_f(e, "Foo", "", &m_on("zz", "Foo => { Foo = 1 }")),
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::BindingNotFound, (5, 11)),
            ][..],
        ),
    ] {
        front_end_row(row, &src, expected);
    }

    // What must stay.
    for (row, src, expected) in [
        (
            "no E0136 for the alternative",
            in_f(e, "Foo", "", &m("Foo | 2 => {}")),
            E_BROKEN,
        ),
        (
            "a write gives no E0001",
            in_main(e, &m("Foo => { Foo = 2 }")),
            E_BROKEN,
        ),
        (
            "a write to a constant gives no E0001",
            in_main(c, &m("print => { print = 2 }")),
            E_CONST,
        ),
        (
            "a write in an if let gives no E0001",
            in_main(
                e,
                "    let o: i64? = None\n    if let Some(Foo) = o { Foo = 2 }",
            ),
            E_BROKEN,
        ),
        (
            "+=",
            in_main(e, &m("Foo => { Foo += 2 }")),
            E_BROKEN,
        ),
        (
            "an index write",
            in_main(e, &m("Foo => { Foo[0] = 2 }")),
            E_BROKEN,
        ),
        (
            "a mut argument gives no E0169",
            format!(
                "{e}fn g(mut x: i64) {{ x = 1 }}\n{}",
                in_main("", &m("Foo => { g(mut Foo) }"))
            ),
            E_BROKEN,
        ),
        (
            "let mut gives no W0002",
            [
                e,
                "enum W { P }\nfn f(w: W) {\n    let mut Foo = w else { return }\n    print(Foo)\n}\nfn main() {}",
            ]
            .concat(),
            E_BROKEN,
        ),
    ] {
        front_end_row(row, &src, expected);
    }

    // What must stay: a true error beside the poisoned name.
    for (row, src, expected) in [
        (
            "an error after the arm",
            in_f(
                e,
                "Foo",
                "",
                &format!("{}\n    let _a: i64 = Foo", m("Foo => {}")),
            ),
            &[(Code::BraceOnNewLine, (2, 1)), (Code::Mismatch, (9, 19))][..],
        ),
        (
            "an error in the else block",
            in_f(
                e,
                "Foo",
                o,
                "    let Some(Foo) = o else {\n        let _b: i64 = Foo\n        return\n    }",
            ),
            &[(Code::BraceOnNewLine, (2, 1)), (Code::Mismatch, (6, 23))][..],
        ),
        (
            "the call form binds no name",
            in_f(e, "Foo", "", &m("Foo(x) => { let _a: i64 = Foo }")),
            &[(Code::BraceOnNewLine, (2, 1)), (Code::Mismatch, (6, 35))][..],
        ),
        (
            "a range bound binds no name",
            in_f(e, "Foo", "", &m("0..Foo => { let _a: i64 = Foo }")),
            &[(Code::BraceOnNewLine, (2, 1)), (Code::Mismatch, (6, 35))][..],
        ),
        (
            "a name that one alternative lacks",
            in_main(e, "    match 1 {\n        Foo | x => print(x)\n    }"),
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::AlternativeBindings, (6, 9)),
            ][..],
        ),
        (
            "a name that one alternative lacks, the other order",
            in_main(e, "    match 1 {\n        x | Foo => print(x)\n    }"),
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::AlternativeBindings, (6, 13)),
            ][..],
        ),
    ] {
        front_end_row(row, &src, expected);
    }
}

/// A type argument after a poisoned name in a pattern: the local that hides it is no value, so
/// the call reports no `E0137`, and an error beside it stays. In each row: the program and its
/// errors.
#[test]
fn a_type_argument_call_of_a_hidden_name_should_give_no_second_error() {
    for (row, src, expected) in [
        (
            "a generic class, a constructor call",
            "class Foo<T>\n{ }\n\nfn f() {\n    match 1 {\n        Foo => {\n print(Foo<i64>(1))\n }\n        _ => {}\n    }\n}\nfn main() {}\n",
            &[(Code::BraceOnNewLine, (2, 1))][..],
        ),
        (
            "a broken enum",
            "enum Foo\n{ A }\n\nfn f() {\n    match 1 {\n        Foo => {\n print(Foo<i64>(1))\n }\n        _ => {}\n    }\n}\nfn main() {}\n",
            &[(Code::BraceOnNewLine, (2, 1))][..],
        ),
        (
            "after let else",
            "enum Foo\n{ A }\n\nfn f(o: i64?) {\n    let Some(Foo) = o else { return }\n print(Foo<i64>(1))\n}\nfn main() {}\n",
            &[(Code::BraceOnNewLine, (2, 1))][..],
        ),
        (
            "a path with type arguments",
            "class Foo<T>\n{ }\n\nfn f() {\n    match 1 {\n        Foo => {\n print(Foo<i64>.make())\n }\n        _ => {}\n    }\n}\nfn main() {}\n",
            &[(Code::BraceOnNewLine, (2, 1))][..],
        ),
        (
            "a broken generic function",
            "fn Foo<T>(x: T)\n{ }\n\nfn f() {\n    match 1 {\n        Foo => {\n print(Foo<i64>(1))\n }\n        _ => {}\n    }\n}\nfn main() {}\n",
            &[(Code::BraceOnNewLine, (2, 1))][..],
        ),
        (
            "a broken alias",
            "type Foo =\n\nfn f() {\n    match 1 {\n        Foo => {\n print(Foo<i64>(1))\n }\n        _ => {}\n    }\n}\nfn main() {}\n",
            &[(Code::Expected, (3, 1))][..],
        ),
        (
            "at an unknown subject, whose E0033 stays",
            "enum Foo\n{ A }\n\nfn f() {\n    match zz {\n        Foo => {\n print(Foo<i64>(1))\n }\n        _ => {}\n    }\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::BindingNotFound, (5, 11)),
            ][..],
        ),
        (
            "over a typed parameter",
            "enum Foo\n{ A }\n\nfn f(Foo: i64) {\n    match 1 {\n        Foo => {\n print(Foo<i64>(1))\n }\n        _ => {}\n    }\n}\nfn main() {}\n",
            &[(Code::BraceOnNewLine, (2, 1))][..],
        ),
        (
            "an error in the arguments stays",
            "enum Foo\n{ A }\n\nfn f() {\n    match 1 {\n        Foo => { let _a = Foo<i64>(zz) }\n        _ => {}\n    }\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::BindingNotFound, (6, 36)),
            ][..],
        ),
        (
            "the misread list gives no E0053",
            "enum Foo\n{ A }\n\nfn g3(a: bool, b: bool) {}\nfn f(x: i64, y: i64) {\n    match 1 {\n        Foo => { g3(Foo<x, y>(1)) }\n        _ => {}\n    }\n}\nfn main() {}\n",
            &[(Code::BraceOnNewLine, (2, 1))][..],
        ),
        (
            "a poisoned constant",
            "const k =\n\nfn f() {\n match 1 {\n k => { let _a = k<i64>(1) }\n _ => {}\n }\n}\nfn main() {}\n",
            &[(Code::Expected, (3, 1))][..],
        ),
        (
            "the hiding local replaces a value of the same scope",
            "enum Foo\n{ A }\n\nfn f(o: i64?) {\n let Foo = 1\n let Some(Foo) = o else { return }\n let _a = Foo<i64>(1)\n}\nfn main() {}\n",
            &[(Code::BraceOnNewLine, (2, 1))][..],
        ),
    ] {
        front_end_row(row, src, expected);
    }
}

/// A local that is a value keeps the `E0137` of a type argument after its name. This holds
/// whatever else is wrong with it. In each row: the program and its errors.
#[test]
#[expect(clippy::too_many_lines, reason = "one row for each case of the table")]
fn a_local_that_is_a_value_should_keep_its_type_argument_error() {
    for (row, src, expected) in [
        (
            "a bare name at a good if let subject",
            "enum Foo\n{ A }\n\nfn f(o: i64?) {\n if let Foo = o { let _a = Foo<i64>(1) }\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::TypeArguments, (5, 28)),
            ][..],
        ),
        (
            "a bare name at a good let else subject",
            "enum Foo\n{ A }\n\nfn f(o: i64?) {\n let Foo = o else { return }\n let _a = Foo<i64>(1)\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::TypeArguments, (6, 11)),
            ][..],
        ),
        (
            "the end of the match ends the hiding",
            "enum Foo\n{ A }\n\nfn f(Foo: i64) {\n    match 1 {\n        Foo => {}\n        _ => {}\n    }\n    let _a = Foo<i64>(1)\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::TypeArguments, (9, 14)),
            ][..],
        ),
        (
            "a later arm has no hiding local",
            "enum Foo\n{ A }\n\nfn f(Foo: i64) {\n match 1 {\n Foo => {}\n _ => { let _a = Foo<i64>(1) }\n }\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::TypeArguments, (7, 18)),
            ][..],
        ),
        (
            "a typed mut parameter",
            "enum Foo\n{ A }\n\nfn f(mut Foo: i64) {\n    let _a = Foo<i64>(1)\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::NeverReassigned, (4, 10)),
                (Code::TypeArguments, (5, 14)),
            ][..],
        ),
        (
            "a failed let mut",
            "enum Foo\n{ A }\n\nfn f() {\n    let mut Foo = zz\n    let _a = Foo<i64>(1)\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::NeverReassigned, (5, 13)),
                (Code::BindingNotFound, (5, 19)),
                (Code::TypeArguments, (6, 14)),
            ][..],
        ),
        (
            "a repeated parameter of a name that is not poisoned",
            "fn f(x: i64, x: i64) {\n    let _a = x<i64>(1)\n}\nfn main() {}\n",
            &[
                (Code::AlreadyDeclared, (1, 14)),
                (Code::TypeArguments, (2, 14)),
            ][..],
        ),
        (
            "a typed let inside the hiding scope",
            "enum Foo\n{ A }\n\nfn f(o: i64?) {\n    if let Some(Foo) = o {\n        let Foo = 1\n        print(Foo<i64>(2))\n    }\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::TypeArguments, (7, 15)),
            ][..],
        ),
        (
            "a typed let after let else",
            "enum Foo\n{ A }\n\nfn f(o: i64?) {\n    let Some(Foo) = o else { return }\n    let Foo = 1\n    print(Foo<i64>(2))\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::TypeArguments, (7, 11)),
            ][..],
        ),
        (
            "a failed let of a name that is not poisoned",
            "enum Foo\n{ A }\n\nfn main() {\n    let x = zz\n    print(x<i64>(2))\n}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::BindingNotFound, (5, 13)),
                (Code::TypeArguments, (6, 11)),
            ][..],
        ),
        (
            "a failed let of a poisoned name",
            "const print = 1\n\nfn main() {\n    let print = zz\n    print(print<i64>(2))\n}\n",
            &[
                (Code::DefinedTwice, (1, 7)),
                (Code::BindingNotFound, (4, 17)),
                (Code::TypeArguments, (5, 11)),
            ][..],
        ),
        (
            "a parameter of an unknown type",
            "enum Foo\n{ A }\n\nfn f(Foo: Zzz) {\n let _a = Foo<i64>(1)\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::UnknownType, (4, 11)),
                (Code::TypeArguments, (5, 11)),
            ][..],
        ),
        (
            "a for over an unknown name",
            "enum Foo\n{ A }\n\nfn f() {\n for Foo in zz {\n let _a = Foo<i64>(1)\n }\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::BindingNotFound, (5, 13)),
                (Code::TypeArguments, (6, 11)),
            ][..],
        ),
        (
            "a broken lower-case function name, a true binding",
            "fn foo<T>(x: T)\n{}\n\nfn f() {\n    match zz {\n        foo => { let _a = foo<i64>(1) }\n        _ => {}\n    }\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::BindingNotFound, (5, 11)),
                (Code::TypeArguments, (6, 27)),
            ][..],
        ),
        (
            "a mut parameter of an unknown type",
            "enum Foo\n{ A }\n\nfn f(mut Foo: Zzz) {\n    let _a = Foo<i64>(1)\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::NeverReassigned, (4, 10)),
                (Code::UnknownType, (4, 15)),
                (Code::TypeArguments, (5, 14)),
            ][..],
        ),
        (
            "a mut parameter of the poisoned type",
            "enum Foo\n{ A }\n\nfn f(mut Foo: Foo) {\n    let _a = Foo<i64>(1)\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::NeverReassigned, (4, 10)),
                (Code::TypeArguments, (5, 14)),
            ][..],
        ),
        (
            "a repeated parameter",
            "enum Foo\n{ A }\n\nfn f(Foo: i64, Foo: i64) {\n    let _a = Foo<i64>(1)\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::AlreadyDeclared, (4, 16)),
                (Code::TypeArguments, (5, 14)),
            ][..],
        ),
        (
            "a repeated mut parameter",
            "enum Foo\n{ A }\n\nfn f(mut Foo: i64, mut Foo: i64) {\n    let _a = Foo<i64>(1)\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::AlreadyDeclared, (4, 24)),
                (Code::TypeArguments, (5, 14)),
            ][..],
        ),
        (
            "a typed parameter, then a repeated mut one",
            "enum Foo\n{ A }\n\nfn f(Foo: i64, mut Foo: i64) {\n let _a = Foo<i64>(1)\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::AlreadyDeclared, (4, 20)),
                (Code::TypeArguments, (5, 11)),
            ][..],
        ),
        (
            "a mut parameter of a poisoned constant's name, a path",
            "const print = 1\n\nfn f(mut print: Zzz) {\n let _a = print<i64>.x(1)\n}\nfn main() {}\n",
            &[
                (Code::DefinedTwice, (1, 7)),
                (Code::NeverReassigned, (3, 10)),
                (Code::UnknownType, (3, 17)),
                (Code::TypeArguments, (4, 11)),
            ][..],
        ),
    ] {
        front_end_row(row, src, expected);
    }
}

/// A poisoned constant in a failed pattern gives no second error for a write to its name, and an
/// intact constant keeps its own. In each row: the program and its errors.
#[test]
fn a_poisoned_constant_in_a_failed_pattern_should_give_no_second_error() {
    for (row, src, expected) in [
        (
            "a write to a rejected constant at an unknown subject",
            "const print = 1\n\nfn f() {\n    match zz {\n        print => { print = 2 }\n        _ => {}\n    }\n}\nfn main() {}\n",
            &[
                (Code::DefinedTwice, (1, 7)),
                (Code::BindingNotFound, (4, 11)),
            ][..],
        ),
        (
            "a write to a broken constant at an unknown subject",
            "const k =\n\nfn f() {\n    match zz {\n        k => { k = 2 }\n        _ => {}\n    }\n}\nfn main() {}\n",
            &[(Code::Expected, (3, 1)), (Code::BindingNotFound, (4, 11))][..],
        ),
        (
            "a write to a broken constant in a variant with a wrong count",
            "const k =\n\nenum E { V(i64) }\nfn f(e: E) {\n    match e {\n        V(k, 1) => { k = 2 }\n        _ => {}\n    }\n}\nfn main() {}\n",
            &[(Code::Expected, (3, 1)), (Code::SubpatternCount, (6, 9))][..],
        ),
        (
            "a let mut of a broken constant with an unknown value",
            "const k =\n\nfn f() {\n    let mut k = zz else { return }\n}\nfn main() {}\n",
            &[(Code::Expected, (3, 1)), (Code::BindingNotFound, (4, 17))][..],
        ),
        (
            "an intact constant keeps its write error",
            "const k = 1\n\nfn f() {\n    match zz {\n        k => { k = 2 }\n        _ => {}\n    }\n}\nfn main() {}\n",
            &[
                (Code::BindingNotFound, (4, 11)),
                (Code::AssignToImmutable, (5, 16)),
            ][..],
        ),
        (
            "a broken function name is a true binding",
            "fn foo()\n{}\n\nfn f() {\n    match zz {\n        foo => { foo = 2 }\n        _ => {}\n    }\n}\nfn main() {}\n",
            &[
                (Code::BraceOnNewLine, (2, 1)),
                (Code::BindingNotFound, (5, 11)),
                (Code::AssignToImmutable, (6, 18)),
            ][..],
        ),
    ] {
        front_end_row(row, src, expected);
    }
}

/// A user's item named `Option` or `Result` leaves the name to the built-in type, as an item
/// named `i64` does: the built-in type and its constructors keep their meaning, so the uses of the
/// name add no error beside `E0026`. In each row: the program and the place of `E0026`.
#[test]
fn an_item_named_option_or_result_should_leave_it_to_the_built_in() {
    with_prelude();
    let uses = "\n\nfn f() -> Result<i64, string> {\n    Ok(1)\n}\n\nfn main() {\n    print(1)\n}";
    for (row, src, at) in [
        ("class Result", format!("class Result {{}}{uses}"), (1, 7)),
        (
            "enum Result",
            format!("enum Result {{\n    A\n}}{uses}"),
            (1, 6),
        ),
        (
            "interface Result",
            format!("interface Result {{}}{uses}"),
            (1, 11),
        ),
        ("constant Result", format!("const Result = 1{uses}"), (1, 7)),
        (
            "class Option",
            "class Option {}\n\nfn f() -> i64? {\n    None\n}\n\nfn main() {\n    let o: Option<i64> = Some(1)\n}"
                .to_string(),
            (1, 7),
        ),
    ] {
        codes_row(row, &src, &[(Code::BuiltInName, at)]);
    }
    // `Ok` names the variant of the built-in `Result`, which has no context here.
    let src = format!("enum Result {{\n    A\n}}\n{}", body("", "let r = Ok(1)"));
    codes_row(
        "enum Result, an Ok with no context",
        &src,
        &[(Code::BuiltInName, (1, 6)), (Code::CannotInfer, (5, 13))],
    );
}

/// Every free function of the prelude's source is a prelude name, whatever its name: the check
/// reads the names from the prelude, not from a list. The prelude here is `fn
/// reserved_by_prelude() {}`.
#[test]
fn a_free_function_of_the_prelude_should_be_a_prelude_name_for_top_level_items() {
    with_prelude_text("fn reserved_by_prelude() {}\n");
    for (item, col) in [
        ("fn reserved_by_prelude() {}", 4),
        ("class reserved_by_prelude {}", 7),
        ("const reserved_by_prelude = 1", 7),
    ] {
        let src = format!("{item}\nfn main() {{}}");
        diagnostic_row(
            item,
            &src,
            Code::DefinedTwice,
            (1, col, "reserved_by_prelude"),
            &[],
        );
    }
    clean(
        "fn f(reserved_by_prelude: i64) {\n    let x = reserved_by_prelude\n    print(x)\n}\nfn main() {}",
    );
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

/// `todo`'s message follows `panic`'s rules, as a statement and as a value. A broken `todo` keeps
/// its type `never`, so as a value it is also "no value". In each row, the body, the warning's
/// position, the other diagnostic, and the body its fix makes, if it has one.
#[test]
fn a_todo_message_should_follow_the_rules_of_panic() {
    // A row ends with whether a `let` holds the call: then it is "no value" too.
    for (statements, warning, (code, pos), fixed, held) in [
        (
            "todo(1)",
            (2, 5),
            (Code::Mismatch, (2, 10)),
            Some("todo(\"{1}\")"),
            false,
        ),
        (
            "todo(\"a\", \"b\")",
            (2, 5),
            (Code::ArgumentCount, (2, 5)),
            None,
            false,
        ),
        (
            "let xs = [1]\n    todo(xs)",
            (3, 5),
            (Code::Mismatch, (3, 10)),
            None,
            false,
        ),
        (
            "let x = todo(1)",
            (2, 13),
            (Code::Mismatch, (2, 18)),
            Some("let x = todo(\"{1}\")"),
            true,
        ),
        (
            "let xs = [1]\n    let x = todo(xs)",
            (3, 13),
            (Code::Mismatch, (3, 18)),
            None,
            true,
        ),
    ] {
        let src = body("", statements);
        let mut wanted = vec![(Code::TodoLeft, warning)];
        if held {
            wanted.push((Code::NoValue, warning));
        }
        wanted.push((code, pos));
        codes(&src, &wanted);
        let all = diags(&src);
        let d = all
            .iter()
            .find(|d| d.code() == code)
            .expect("the row should report its code");
        match fixed {
            Some(fixed) => {
                assert_eq!(applied(&src, d), body("", fixed));
                fix_removes_its_code(&src, d);
            }
            None => assert_eq!(fixes(&src, d), [], "{src}"),
        }
    }
    // As a value, the broken call keeps its type `never`, which a `let` cannot hold.
    codes(
        &body("", "let x = todo(\"a\", \"b\")"),
        &[
            (Code::TodoLeft, (2, 13)),
            (Code::ArgumentCount, (2, 13)),
            (Code::NoValue, (2, 13)),
        ],
    );
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

/// A labelled message is reported once, and the `todo` keeps its type `never`, as a `panic` does.
#[test]
fn a_todo_with_a_label_should_keep_its_type() {
    codes(
        &body("", "let x = todo(m: \"x\")"),
        &[
            (Code::TodoLeft, (2, 13)),
            (Code::NoValue, (2, 13)),
            (Code::LabeledArgument, (2, 18)),
        ],
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

/// `dbg(x)` has the type of `x`. So it needs its context exactly when `x` does: a `Result` or
/// a generic call around it hands that context to `x`.
#[test]
fn dbg_should_wait_for_its_context_when_its_argument_does() {
    with_prelude();
    for statement in [
        "let v: Result<[f64], [char]> = Ok(dbg([]))",
        "let v: Result<i64, [f64]> = Err(dbg([]))",
        "let v: Result<[f64], i64> = Ok(dbg(dbg([])))",
        "let v: [f64] = id(dbg([]))",
        "let v: Result<i64, string> = Ok(dbg(1))",
    ] {
        let src = format!("{}\nfn id<T>(x: T) -> T {{ x }}", body("", statement));
        clean_row(statement, &src);
    }
}

/// `dbg(e)` passes its context to `e`, so `f(dbg(e))` and `f([dbg(e)])` get the context that
/// `f(e)` and `f([e])` get and report the same diagnostics. Each diagnostic is its code and the
/// source text under its span, so a diagnostic at the wrong place fails. The spans of the two
/// programs differ by the width of `dbg(`, which is why they are compared as text.
#[test]
fn dbg_should_give_a_call_the_diagnostics_of_its_argument() {
    with_prelude();
    for (param, plain, through_dbg) in [
        ("[T]", "[]", "dbg([])"),
        ("[T]", "[]", "dbg(dbg([]))"),
        ("[T]", "[[]]", "dbg([[]])"),
        ("[T]", "[[]]", "[dbg([])]"),
        ("[T]", "[[]]", "[[dbg([])]]"),
        ("[T]", "[[], []]", "[dbg([]), dbg([])]"),
        ("[T]", "[[]]", "[dbg(dbg([]))]"),
        ("[T]", "[[], 1]", "[dbg([]), dbg(1)]"),
        ("[T]", "[1, []]", "[dbg(1), dbg([])]"),
        ("[T]", "[[], 1]", "[dbg([]), 1]"),
        ("[T?]", "[None, 1]", "[dbg(None), dbg(1)]"),
        ("[T]", "[1, None]", "[dbg(1), dbg(None)]"),
        ("[T]", "[1, \"s\"]", "[dbg(1), \"s\"]"),
        ("[[T]]", "[[]]", "dbg([[]])"),
        ("T?", "None", "dbg(None)"),
        ("T?", "Some(None)", "dbg(Some(None))"),
        ("[T?]", "[None]", "[dbg(None)]"),
        ("Result<T, bool>", "Ok(1)", "dbg(Ok(1))"),
        ("Result<T, bool>", "Ok(1)", "dbg(dbg(Ok(1)))"),
        ("[Result<T, bool>]", "[Ok(1)]", "[dbg(Ok(1))]"),
        ("Result<[f64], T>", "Err(1)", "dbg(Err(1))"),
        ("Result<[T], bool>", "Ok([])", "dbg(Ok([]))"),
    ] {
        let program = |arg: &str| {
            let src = format!(
                "fn f<T>(x: {param}) -> i64 {{ 0 }}\nfn main() {{\n    let v = f({arg})\n}}"
            );
            let found: Vec<_> = diags(&src)
                .iter()
                .map(|d| (d.code(), src[d.span().range()].to_string()))
                .collect();
            (found, src)
        };
        let (plain_found, _) = program(plain);
        let (found, src) = program(through_dbg);
        assert_eq!(found, plain_found, "row `{param}`, `{through_dbg}`: {src}");
    }
}

/// `dbg(e)` passes its expected type to `e`, so it has the order of `e` among the elements of an
/// array literal, the branches of an `if` and the operands of `==`. A `dbg([])` is then checked
/// last, with the type of the others, and gives the diagnostics of `[]`: one error for a literal
/// of empty arrays, and a mismatch beside another type. Each diagnostic is its code and its
/// line, as the columns of the two programs differ by the width of `dbg(`.
#[test]
fn dbg_should_have_the_order_of_its_argument() {
    with_prelude();
    for (plain, through_dbg) in [
        ("let a = [[], []]", "let a = [dbg([]), dbg([])]"),
        ("let a = [[], []]", "let a = [dbg(dbg([])), dbg([])]"),
        ("let a = [None, None]", "let a = [dbg(None), dbg(None)]"),
        ("let a = [1, None]", "let a = [dbg(1), dbg(None)]"),
        ("let a = [None, 1]", "let a = [dbg(None), dbg(1)]"),
        ("let a = [[], 1]", "let a = [dbg([]), dbg(1)]"),
        ("let a = []", "let a = dbg(dbg([]))"),
        ("let a: [i64] = []", "let a: [i64] = dbg([])"),
        (
            "let a: [[i64]] = [[], [1]]",
            "let a: [[i64]] = [dbg([]), dbg([1])]",
        ),
        (
            "let a = if true { None } else { 1 }",
            "let a = if true { dbg(None) } else { 1 }",
        ),
        (
            "let a = if true { 1 } else { None }",
            "let a = if true { 1 } else { dbg(None) }",
        ),
        (
            "let a = if true { [] } else { 1 }",
            "let a = if true { dbg([]) } else { 1 }",
        ),
        (
            "let x: i64? = None\n    let a = None == x",
            "let x: i64? = None\n    let a = dbg(None) == x",
        ),
        (
            "let x: i64? = None\n    let a = x == None",
            "let x: i64? = None\n    let a = x == dbg(None)",
        ),
        (
            "let x: [i64] = [1]\n    let a = [] == x",
            "let x: [i64] = [1]\n    let a = dbg([]) == x",
        ),
        ("let a = None == []", "let a = dbg(None) == dbg([])"),
    ] {
        let program = |statement: &str| {
            let src = format!("fn main() {{\n    {statement}\n    print(a)\n}}");
            let found: Vec<_> = diags(&src)
                .iter()
                .map(|d| (d.code(), at(&src, d.span()).0))
                .collect();
            (found, src)
        };
        let (plain_found, _) = program(plain);
        let (found, src) = program(through_dbg);
        assert_eq!(found, plain_found, "row `{through_dbg}`: {src}");
    }
}

/// A call of `dbg` with a type argument or another count of arguments than one is no `dbg(x)`,
/// so it keeps the order of a call: its `[]` is checked first, with no expected type, and gives
/// its own error beside the one of the call.
#[test]
fn dbg_of_another_shape_should_keep_the_order_of_a_call() {
    with_prelude();
    for (statement, expected) in [
        ("let _ = [dbg([], 1), 1]", &[Code::PrintArgument][..]),
        ("let _ = [dbg(), 1]", &[Code::PrintArgument][..]),
        ("let _ = [dbg<i64>(1), 1]", &[Code::TypeArguments][..]),
        (
            "let _ = [dbg<i64>([]), 1]",
            &[Code::TypeArguments, Code::CannotInfer][..],
        ),
    ] {
        let src = body("", statement);
        let found: Vec<_> = diags(&src).iter().map(Diagnostic::code).collect();
        assert_eq!(found, expected, "row `{statement}`: {src}");
    }
}

/// A label on the one argument of `dbg` is an error (E0055), and the argument still waits for
/// its context as without the label: the label adds no E0050, and the code after it is the one
/// that the form without the label gives (#327). Each row has a `[]` or `None` that nothing but
/// a sibling can type.
#[test]
fn a_labeled_dbg_should_give_the_errors_of_the_form_without_the_label() {
    with_prelude();
    let fn_p = "fn p<T>(a: [T], b: T) -> i64 { 1 }\n";
    for (labeled, plain) in [
        ("print([dbg(x: []), 1])", "print([dbg([]), 1])"),
        (
            "print([dbg(x: None), Some(1)])",
            "print([dbg(None), Some(1)])",
        ),
        ("print([dbg(x: []), [1]])", "print([dbg([]), [1]])"),
        ("let a = [dbg(x: []), [1]]", "let a = [dbg([]), [1]]"),
        ("print(p(dbg(x: []), 1))", "print(p(dbg([]), 1))"),
        ("let xs: [i64] = dbg(x: [])", "let xs: [i64] = dbg([])"),
    ] {
        let run = |statement: &str| {
            let src = format!("{fn_p}fn main() {{\n    {statement}\n}}");
            let found: Vec<_> = diags(&src).iter().map(Diagnostic::code).collect();
            (found, src)
        };
        let (mut want, _) = run(plain);
        want.insert(0, Code::LabeledArgument);
        let (found, src) = run(labeled);
        assert_eq!(found, want, "row `{labeled}`: {src}");
    }
}

/// A labeled `dbg` has a poisoned result, so it binds no `T`. The `[]` of a later argument that
/// is a labeled `dbg` too is a hole to skip, as it is without the label. The two labels are the
/// only errors: the `[]` was not a hole, and the call reported a mismatch with the hole `_`.
#[test]
fn a_labeled_dbg_array_beside_a_labeled_dbg_should_add_no_mismatch() {
    with_prelude();
    let src = format!(
        "fn q<T>(a: T, b: [T]) {{}}\n{}",
        body("", "q(dbg(x: 1), [dbg(x: [])])")
    );
    let found: Vec<_> = diags(&src).iter().map(Diagnostic::code).collect();
    assert_eq!(
        found,
        [Code::LabeledArgument, Code::LabeledArgument],
        "{src}"
    );
}

/// A second label, or a second argument, makes `dbg` a call of another shape: each error stays.
/// A call of `print` with a label is not a `dbg`: its `[]` has no context in either form.
#[test]
fn a_labeled_dbg_with_more_than_one_argument_should_keep_all_its_errors() {
    with_prelude();
    let src = body("", "print([dbg(x: 1, y: 2), 1])");
    let found: Vec<_> = diags(&src).iter().map(Diagnostic::code).collect();
    assert_eq!(
        found,
        [
            Code::PrintArgument,
            Code::LabeledArgument,
            Code::LabeledArgument
        ],
        "{src}"
    );
    let src = body("", "print(x: [])");
    let found: Vec<_> = diags(&src).iter().map(Diagnostic::code).collect();
    assert_eq!(found, [Code::LabeledArgument, Code::CannotInfer], "{src}");
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

/// `Ok` and `Err` name the variants of a `Result` that declares them: with no prelude, a call of
/// either names nothing.
#[test]
fn ok_should_name_nothing_when_no_result_declares_it() {
    codes(
        &body("", "let r = Ok(1)"),
        &[(Code::FunctionNotFound, (2, 13))],
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

/// A `dbg` in a function of the prelude holds the text of its argument, which the checker reads
/// from the prelude's text and not from the user's file: a span of the prelude lies outside the
/// user's text. The program calls the function, which a prelude with a method, a free function, a
/// class method, or a string literal in the body gives.
#[test]
fn dbg_in_a_prelude_function_should_hold_the_text_of_its_argument() {
    for (prelude, main, part) in [
        (
            "extend i64 {\n    fn probe(self) -> i64 {\n        dbg(self)\n        self\n    }\n}\n",
            "fn main() {\n    print(5.probe())\n}",
            "(dbg \"self\"",
        ),
        (
            "fn probe(n: i64) -> i64 {\n    dbg(n +   1)\n    n\n}\n",
            "fn main() {\n    print(probe(5))\n}",
            "(dbg \"n +   1\"",
        ),
        (
            "class Probe {\n    n: i64\n    fn probe(self) -> i64 {\n        dbg(self.n)\n        self.n\n    }\n}\n",
            "fn main() {\n    print(Probe(n: 5).probe())\n}",
            "(dbg \"self.n\"",
        ),
        (
            "fn probe() -> string {\n    dbg(\"text\")\n}\n",
            "fn main() {\n    print(probe())\n}",
            "(dbg \"\\\"text\\\"\"",
        ),
    ] {
        let (program, ds) = front_end::checked_with(prelude, main);
        assert!(ds.is_empty(), "{prelude}: {ds:?}");
        let printed = program.expect("no errors").to_string();
        assert!(printed.contains(part), "{part} in {printed}");
    }
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
        ("a statement", "dbg(f(1))\n    dbg(dbg(2))\n    print(1)"),
    ] {
        let src = format!("fn f(x: i64) -> i64 {{\n    x\n}}\nfn main() {{\n    {statements}\n}}");
        clean_row(row, &src);
    }
}

/// `dbg` reports an argument that has no value, never finishes or is not one as `print` does.
#[test]
fn dbg_should_report_the_argument_errors_of_print() {
    for (statement, code, pos) in [
        ("dbg(greet())", Code::PrintArgument, (3, 9)),
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

/// The `min`, `max` and `clamp` of `i64` are the prelude's: a misspelled name suggests them, and
/// a wrong count of arguments is the general call-count code.
#[test]
fn an_i64_method_of_the_prelude_should_keep_its_suggestion_and_call_count_code() {
    with_prelude();
    for (written, method) in [("mim", "min"), ("maxx", "max"), ("clam", "clamp")] {
        let src = body("n: i64", &format!("let _ = n.{written}(3)"));
        suggests(&src, Code::NoMember, (2, 15, written), method);
    }
    for call in ["n.min()", "n.max(1, 2)", "n.clamp(1)"] {
        codes(
            &body("n: i64", &format!("let _ = {call}")),
            &[(Code::ArgumentCount, (2, 13))],
        );
    }
}

/// The `sum` of `[i64]` and `[f64]` is the prelude's: a misspelled name suggests it, a wrong
/// count of arguments is the general call-count code, and an array that no block covers is E0069,
/// whatever the array holds.
#[test]
fn an_array_sum_of_the_prelude_should_keep_its_suggestion_and_codes() {
    with_prelude();
    for (params, written) in [("xs: [i64]", "summ"), ("xs: [f64]", "sm")] {
        let src = body(params, &format!("let _ = xs.{written}()"));
        suggests(&src, Code::NoMember, (2, 16, written), "sum");
    }
    codes(
        &body("xs: [i64]", "let _ = xs.sum(1)"),
        &[(Code::ArgumentCount, (2, 13))],
    );
    for params in ["xs: [string]", "xs: [bool]", "xs: [[i64]]", "xs: [i64?]"] {
        codes_row(
            params,
            &body(params, "let _ = xs.sum()"),
            &[(Code::SumNeedsNumbers, (2, 16))],
        );
    }
    // A type parameter is no `i64` or `f64`.
    codes(
        "fn f<T>(xs: [T]) {\n    let _ = xs.sum()\n}\nfn main() {}",
        &[(Code::SumNeedsNumbers, (2, 16))],
    );
}

/// A method of the prelude that no block gives the array, written as a function, is the
/// missing-method error of the method call `xs.sum()` and not a missing function. A number or a
/// name that is no array method keeps E0052; an array that has the method keeps the method-call
/// fix.
#[test]
fn a_method_written_as_a_function_should_report_the_array_that_lacks_it() {
    with_prelude();
    for (row, params, call, code) in [
        (
            "an array of u8",
            "xs: [u8]",
            "sum(xs)",
            Code::SumNeedsNumbers,
        ),
        (
            "an optional array",
            "xs: [u8]?",
            "sum(xs)",
            Code::SumNeedsNumbers,
        ),
        ("a number", "xs: [u8]", "sum(5)", Code::FunctionNotFound),
        (
            "an unknown name",
            "xs: [u8]",
            "total(xs)",
            Code::FunctionNotFound,
        ),
        (
            "an array that has it",
            "xs: [i64]",
            "sum(xs)",
            Code::MethodAsFunction,
        ),
    ] {
        let src = body(params, &format!("let _ = {call}"));
        codes_row(row, &src, &[(code, (2, 13))]);
    }
    // The other arguments are still checked, once.
    codes(
        &body("xs: [u8]", "let _ = sum(xs, missing)"),
        &[
            (Code::SumNeedsNumbers, (2, 13)),
            (Code::BindingNotFound, (2, 21)),
        ],
    );
}

/// A user item takes the name of an identifier of the prelude: the prelude's bodies still check
/// without a diagnostic. `front_end::checked` stops with an internal error on a diagnostic
/// that points into the prelude, so a call that returns is the proof. A name such as `Option`
/// gives a diagnostic in the user's file: that is fine.
#[test]
fn prelude_should_check_clean_beside_a_user_item_of_any_name_it_uses() {
    let mut names = Vec::new();
    for source in [LANG_SOURCE, PRELUDE_SOURCE] {
        let lexed = lexer::lex(source).expect("the prelude should lex");
        for token in lexed.tokens {
            if let TokenKind::Ident(name) = token.kind
                && !names.contains(&name)
            {
                names.push(name);
            }
        }
    }
    assert!(names.len() > 10, "{names:?}");
    for name in names {
        for item in [
            format!("enum {name} {{ A }}"),
            format!("class {name} {{}}"),
            format!("fn {name}() {{}}"),
            format!("const {name}: i64 = 1"),
            format!("interface {name} {{}}"),
        ] {
            front_end::checked(&format!("{item}\nfn main() {{}}"));
        }
    }
}

/// A diagnostic about a `match` in the prelude reads the text around the arms, with offsets
/// that start at `PRELUDE_BASE`. The check then stops with an internal error, as for every
/// diagnostic in the prelude, and does not panic on a slice of the user's text.
#[test]
fn match_in_the_prelude_should_give_an_internal_error_when_it_is_wrong() {
    for (row, prelude) in [
        (
            "an unreachable arm",
            "fn probe(n: bool) -> i64 {\n    match n {\n        true => 1\n        false => 2\n        true => 3\n    }\n}\n",
        ),
        (
            "a missing case",
            "fn probe(n: bool) -> i64 { match n { true => 1 } }\n",
        ),
        (
            "a missing case on its own lines",
            "fn probe(n: bool) -> i64 {\n    match n {\n        true => 1\n    }\n}\n",
        ),
    ] {
        let caught = std::panic::catch_unwind(|| front_end::checked_with(prelude, "fn main() {}"));
        let message = caught.expect_err(row);
        let message = message
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| message.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        assert!(
            message.starts_with("internal compiler error: the prelude does not check clean"),
            "{row}: {message}"
        );
    }
}
