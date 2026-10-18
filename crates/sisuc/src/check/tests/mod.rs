use super::*;
use crate::diagnostic::explanations::apply;
use crate::diagnostic::{Code, Fix};
use crate::parser::parse_ok;
use crate::source::line_col;

mod arrays;
mod chars;
mod constants;
mod enums;
mod floats;
mod for_loops;
mod patterns;
mod places;
mod prelude;
mod shadowing;
mod strings;
mod u8;
mod unused;

pub(super) fn checked(src: &str) -> (Option<tir::Program>, Vec<Diagnostic>) {
    check(&parse_ok(src), src)
}

fn diags(src: &str) -> Vec<Diagnostic> {
    checked(src).1
}

/// The typed tree of a program that checks without a diagnostic.
fn lowered(src: &str) -> String {
    let (program, ds) = checked(src);
    assert!(ds.is_empty(), "{ds:?}");
    program.expect("no errors").to_string()
}

/// Asserts that `src` checks clean and its `tir` contains each of `parts`.
fn contains(src: &str, parts: &[&str]) {
    let printed = lowered(src);
    for part in parts {
        assert!(printed.contains(part), "{part} in {printed}");
    }
}

/// The diagnostics are exactly these, in order: `(code, (line, col))`; no `tir` if one is an
/// error.
fn codes(src: &str, expected: &[(Code, (usize, usize))]) {
    let (program, ds) = checked(src);
    let found: Vec<_> = ds.iter().map(|d| (d.code(), at(src, d.span()))).collect();
    assert_eq!(found, expected);
    let failed = expected
        .iter()
        .any(|(c, _)| c.severity() == Severity::Error);
    assert_eq!(
        program.is_none(),
        failed,
        "only an error withholds the `tir`"
    );
}

/// A diagnostic's code, its primary position and its secondary positions.
type Found = (Code, (usize, usize), Vec<(usize, usize)>);

/// Each diagnostic of `src`, in order.
fn found(src: &str) -> Vec<Found> {
    let (_, ds) = checked(src);
    ds.iter()
        .map(|d| {
            let secondary = d.secondary().iter().map(|(s, _)| at(src, *s)).collect();
            (d.code(), at(src, d.span()), secondary)
        })
        .collect()
}

/// Exactly one diagnostic, of `code`, whose primary span is `pos` = (line, col, text) and whose
/// secondary spans start at `secondary`.
fn diagnostic(
    src: &str,
    code: Code,
    pos: (usize, usize, &str),
    secondary: &[(usize, usize)],
) -> Diagnostic {
    let (program, mut ds) = checked(src);
    assert_eq!(ds.len(), 1, "{ds:?}");
    let d = ds.remove(0);
    assert_eq!(d.code(), code);
    assert_eq!(
        program.is_none(),
        code.severity() == Severity::Error,
        "only an error withholds the `tir`"
    );
    let span = d.span();
    assert_eq!((at(src, span), &src[span.range()]), ((pos.0, pos.1), pos.2));
    let found: Vec<_> = d.secondary().iter().map(|(s, _)| at(src, *s)).collect();
    assert_eq!(found, secondary);
    d
}

/// A fix as (safe, [(line, col, replaced text, replacement)]).
type FixAt<'a> = (bool, Vec<(usize, usize, &'a str, String)>);

/// `d`'s fixes, located in `src`.
fn fixes<'a>(src: &'a str, d: &Diagnostic) -> Vec<FixAt<'a>> {
    d.fixes()
        .iter()
        .map(|f| {
            let edits = f
                .edits()
                .iter()
                .map(|e| {
                    let s = e.span();
                    let (line, col) = at(src, s);
                    (line, col, &src[s.range()], e.replacement().to_string())
                })
                .collect();
            (f.safe(), edits)
        })
        .collect()
}

/// Asserts that applying `d`'s safe fixes to `src` leaves no diagnostic of `d`'s code.
fn fix_removes_its_code(src: &str, d: &Diagnostic) {
    let edits: Vec<_> = d
        .fixes()
        .iter()
        .filter(|f| f.safe())
        .flat_map(Fix::edits)
        .collect();
    assert!(!edits.is_empty(), "{d:?} has no safe fix");
    let result = apply(src, &edits);
    let left: Vec<_> = diags(&result).iter().map(Diagnostic::code).collect();
    assert!(!left.contains(&d.code()), "{result}: {left:?}");
}

/// `src` with every fix of `d` applied, which must still lex and parse.
fn applied(src: &str, d: &Diagnostic) -> String {
    let edits: Vec<_> = d.fixes().iter().flat_map(Fix::edits).collect();
    let result = apply(src, &edits);
    parse_ok(&result);
    result
}

fn at(src: &str, span: Span) -> (usize, usize) {
    line_col(src, span.start())
}

fn clean(src: &str) {
    let ds = diags(src);
    assert!(ds.is_empty(), "{ds:?}");
}

#[test]
fn duplicate_function() {
    diagnostic(
        "fn main() {}\nfn main() {}",
        Code::DefinedTwice,
        (2, 4, "main"),
        &[(1, 4)],
    );
}

#[test]
fn missing_main() {
    let src = "fn f() {}";
    diagnostic(src, Code::NoMain, (1, 1, ""), &[]);
    assert_eq!(diags(src)[0].span(), Span::new(0, 0));
}

/// `main` takes no parameters or one `[string]` of any name, and returns no value. A type that
/// does not resolve is reported once, as unknown, and not again as a wrong signature.
#[test]
fn main_signatures() {
    let signature = (Code::MainSignature, (1, 4));
    for (src, expected) in [
        ("fn main() {}", vec![]),
        ("fn main(args: [string]) {}", vec![]),
        ("fn main(xs: [string]) {}", vec![]),
        ("fn main(argc: i64, argv: [string]) {}", vec![signature]),
        ("fn main() -> i64 { 0 }", vec![signature]),
        ("fn main(args: [i64]) {}", vec![signature]),
        ("fn main(x: i64) {}", vec![signature]),
        ("fn main(args: [string]?) {}", vec![signature]),
        (
            "fn main(args: [Bogus]) {}",
            vec![(Code::UnknownType, (1, 16))],
        ),
        ("fn main() -> Bogus {}", vec![(Code::UnknownType, (1, 14))]),
    ] {
        codes(src, &expected);
    }
}

#[test]
fn unknown_type() {
    diagnostic(
        "fn main() { let x: str = 1 }",
        Code::UnknownType,
        (1, 20, "str"),
        &[],
    );
}

#[test]
fn unknown_name() {
    diagnostic(
        "fn main() { print(y) }",
        Code::BindingNotFound,
        (1, 19, "y"),
        &[],
    );
}

#[test]
fn unknown_function() {
    diagnostic(
        "fn main() { g() }",
        Code::FunctionNotFound,
        (1, 13, "g"),
        &[],
    );
}

#[test]
fn shadow_in_block() {
    contains(
        "fn main() {\n    let x = 1\n    let x = 2\n}",
        &["(let x#0 1)", "(let x#1 2)"],
    );
}

#[test]
fn repeated_param() {
    diagnostic(
        "fn f(a: i64, a: i64) {}\nfn main() {}",
        Code::AlreadyDeclared,
        (1, 14, "a"),
        &[(1, 6)],
    );
}

/// A repeated parameter is poisoned: only the repetition is reported, not an assignment to it
/// nor a use of it.
#[test]
fn repeated_param_is_poisoned() {
    codes(
        "fn f(a: i64, a: i64) {\n    a = 1\n    print(a + true)\n}\nfn main() {}",
        &[(Code::AlreadyDeclared, (1, 14))],
    );
}

#[test]
fn shadowing_in_inner_block() {
    let src = "fn main() {\n    let x = 1\n    if true {\n        let x = true\n        print(x)\n    }\n    print(x)\n}";
    assert!(diags(src).is_empty());
}

#[test]
fn functions_in_any_order() {
    assert!(diags("fn main() { print(f(1)) }\nfn f(n: i64) -> i64 { n }").is_empty());
}

#[test]
fn variable_may_shadow_a_param() {
    let src = "fn f(n: i64) {\n    let mut n = n\n    n = 1\n}\nfn main() {}";
    clean(src);
}

#[test]
fn every_error_in_one_run() {
    codes(
        "fn main() {\n    print(a)\n    let b: bool = 1\n    print(1 + true)\n}",
        &[
            (Code::BindingNotFound, (2, 11)),
            (Code::Mismatch, (3, 19)),
            (Code::Mismatch, (4, 15)),
        ],
    );
}

#[test]
fn poisoned_binding_reports_once() {
    codes(
        "fn main() {\n    let x = nope\n    print(x + 1)\n}",
        &[(Code::BindingNotFound, (2, 13))],
    );
}

#[test]
fn poisoned_operand_checks_siblings() {
    codes(
        "fn f(a: i64, b: i64) {}\nfn main() {\n    f(nope, 1 + true)\n}",
        &[(Code::BindingNotFound, (3, 7)), (Code::Mismatch, (3, 17))],
    );
}

#[test]
fn if_with_failed_branch_takes_other_type() {
    codes(
        "fn main() {\n    let y = if true { nope } else { 1 }\n    print(y + true)\n}",
        &[(Code::BindingNotFound, (2, 23)), (Code::Mismatch, (3, 15))],
    );
}

#[test]
fn unknown_let_type_poisons() {
    codes(
        "fn main() {\n    let x: str = 1\n    print(x + true)\n}",
        &[(Code::UnknownType, (2, 12))],
    );
}

#[test]
fn unknown_param_type_still_checks_body() {
    codes(
        "fn f(a: str) -> i64 { true }\nfn main() { let _ = f(1) }",
        &[(Code::UnknownType, (1, 9)), (Code::Mismatch, (1, 23))],
    );
}

#[test]
fn duplicate_checked_against_own_signature() {
    codes(
        "fn f(a: i64) {}\nfn f(b: bool) -> bool { b }\nfn main() {}",
        &[(Code::DefinedTwice, (2, 4))],
    );
}

#[test]
fn duplicate_call_resolves_to_first() {
    codes(
        "fn f(a: i64) -> i64 { a }\nfn f(b: bool) -> bool { b }\nfn main() {\n    print(f(1))\n}",
        &[(Code::DefinedTwice, (2, 4))],
    );
}

#[test]
fn failed_call_still_checks_arguments() {
    let rows = [
        (
            "unknown function",
            "fn main() {\n    g(1 + true)\n}",
            [(Code::FunctionNotFound, (2, 5)), (Code::Mismatch, (2, 11))],
        ),
        (
            "arity",
            "fn f(a: i64) {}\nfn main() {\n    f(1 + true, 2)\n}",
            [(Code::ArgumentCount, (3, 5)), (Code::Mismatch, (3, 11))],
        ),
        (
            "print arity",
            "fn main() {\n    print(1 + true, 2)\n}",
            [(Code::PrintArgument, (2, 5)), (Code::Mismatch, (2, 15))],
        ),
    ];
    for (name, src, expected) in rows {
        println!("row: {name}");
        codes(src, &expected);
    }
}

#[test]
fn duplicate_body_still_checked() {
    codes(
        "fn f() {}\nfn f() { print(nope) }\nfn main() {}",
        &[
            (Code::DefinedTwice, (2, 4)),
            (Code::BindingNotFound, (2, 16)),
        ],
    );
}

/// The `main` rejected as a duplicate still exists; only the duplicate is reported.
#[test]
fn main_after_a_class_named_main_is_not_missing() {
    codes(
        "class main {}\nfn main() {}",
        &[(Code::DefinedTwice, (2, 4))],
    );
}

#[test]
fn missing_main_and_body_error() {
    codes(
        "fn f() { print(nope) }",
        &[(Code::NoMain, (1, 1)), (Code::BindingNotFound, (1, 16))],
    );
}

#[test]
fn duplicate_body_uses_own_return_type() {
    codes(
        "fn f() -> i64 { 1 }\nfn f() -> bool { true }\nfn main() {}",
        &[(Code::DefinedTwice, (2, 4))],
    );
}

#[test]
fn if_with_failed_and_never_branches_is_error() {
    codes(
        "fn main() {\n    let c = true\n    let x = if c { nope } else { return }\n    print(1)\n}",
        &[(Code::BindingNotFound, (3, 20))],
    );
}

/// A rule that fails makes its expression `Error`. Each row binds the failed expression
/// and uses it where a typed result would cascade into a second error.
#[test]
fn failed_rule_makes_its_expression_error() {
    const MISMATCH: Code = Code::Mismatch;
    const F: &str = "fn f(a: i64) -> i64 { a }\n";
    let rows: [(&str, String, Code, (usize, usize)); 12] = [
        (
            "unary",
            "fn main() {\n    let x = -true\n    print(x == false)\n}".into(),
            MISMATCH,
            (2, 14),
        ),
        (
            "binary lhs",
            "fn main() {\n    let x = true + 1\n    print(x == false)\n}".into(),
            MISMATCH,
            (2, 13),
        ),
        (
            "binary rhs",
            "fn main() {\n    let x = 1 + true\n    print(x == false)\n}".into(),
            MISMATCH,
            (2, 17),
        ),
        (
            "compare",
            "fn main() {\n    let x = 1 < true < 2\n    print(x + 1)\n}".into(),
            MISMATCH,
            (2, 17),
        ),
        (
            "equal",
            "fn main() {\n    let x = 1 == true\n    print(x + 1)\n}".into(),
            Code::CannotCompare,
            (2, 13),
        ),
        (
            "call arity",
            format!("{F}fn main() {{\n    let x = f()\n    print(x == false)\n}}"),
            Code::ArgumentCount,
            (3, 13),
        ),
        (
            "call argument",
            format!("{F}fn main() {{\n    let x = f(true)\n    print(x == false)\n}}"),
            MISMATCH,
            (3, 15),
        ),
        (
            "print arity",
            "fn main() {\n    let x = print(1, 2)\n    print(x)\n}".into(),
            Code::PrintArgument,
            (2, 13),
        ),
        (
            "print unit argument",
            "fn main() {\n    let x = print(print(1))\n    print(x)\n}".into(),
            Code::PrintArgument,
            (2, 13),
        ),
        (
            "if branches",
            "fn main() {\n    let x = if true { 1 } else { false }\n    print(x == false)\n}"
                .into(),
            MISMATCH,
            (2, 34),
        ),
        (
            "if condition with else",
            "fn main() {\n    let x = if 1 { 2 } else { 3 }\n    print(x == false)\n}".into(),
            MISMATCH,
            (2, 16),
        ),
        (
            "if condition without else",
            "fn main() {\n    let x = if 1 { print(2) }\n    print(x)\n}".into(),
            MISMATCH,
            (2, 16),
        ),
    ];
    for (name, src, code, pos) in rows {
        println!("row: {name}");
        codes(&src, &[(code, pos)]);
    }
}

#[test]
fn warning_only_program_lowers() {
    let src = "fn main() {\n    let mut x = 1\n    print(x)\n}";
    codes(src, &[(Code::NeverReassigned, (2, 13))]);
}

#[test]
fn warning_and_error_in_position_order() {
    // The warning is found when the scope closes, after the error.
    codes(
        "fn main() {\n    let mut x = 1\n    print(nope)\n}",
        &[
            (Code::NeverReassigned, (2, 13)),
            (Code::BindingNotFound, (3, 11)),
        ],
    );
}

#[test]
fn trailing_expression_is_the_block_value() {
    let (program, ds) =
        checked("fn f() -> i64 {\n    let x = 1\n    x\n}\nfn main() {\n    let y = f()\n}");
    assert!(ds.is_empty(), "{ds:?}");
    let program = program.expect("no errors");
    let f = &program.functions[0].body;
    assert!(matches!(f.stmts.as_slice(), [tir::Stmt::Let { .. }]));
    assert!(matches!(
        f.value.as_deref(),
        Some(tir::Expr {
            kind: tir::ExprKind::Local(_),
            ..
        })
    ));
    let main = &program.functions[1].body;
    assert!(matches!(main.stmts.as_slice(), [tir::Stmt::Let { .. }]));
    assert!(main.value.is_none());
}

/// A `while` lowers to an expression statement, but it is not an expression in `ast`, so
/// a block that ends in one has no value.
#[test]
fn loop_tail_is_not_the_block_value() {
    let (program, ds) =
        checked("fn main() {\n    let mut i = 0\n    while i < 3 {\n        i += 1\n    }\n}");
    assert!(ds.is_empty(), "{ds:?}");
    let main = &program.expect("no errors").functions[0].body;
    assert!(matches!(
        main.stmts.as_slice(),
        [tir::Stmt::Let { .. }, tir::Stmt::Expr(_)]
    ));
    assert!(main.value.is_none());
}

#[test]
fn lowers_while_to_loop() {
    assert_eq!(
        lowered("fn main() {\n    let mut i = 0\n    while i < 3 {\n        i = i + 1\n    }\n}"),
        "(fn main () unit (block (let mut i#0 0) (loop (block (if (< i#0 3) (block (= i#0 (+ i#0 1))) (block (break)))))))"
    );
}

#[test]
fn lowers_continue() {
    assert_eq!(
        lowered(
            "fn main() {\n    let mut i = 0\n    while i < 3 {\n        i = i + 1\n        if i == 2 { continue }\n        print(i)\n    }\n}"
        ),
        "(fn main () unit (block (let mut i#0 0) (loop (block (if (< i#0 3) (block (= i#0 (+ i#0 1)) (if (== i#0 2) (block (continue))) (print i#0)) (block (break)))))))"
    );
}

#[test]
fn break_outside_loop() {
    codes("fn main() { break }", &[(Code::JumpOutsideLoop, (1, 13))]);
}

#[test]
fn continue_outside_loop() {
    codes(
        "fn main() { continue }",
        &[(Code::JumpOutsideLoop, (1, 13))],
    );
}

#[test]
fn break_after_loop() {
    codes(
        "fn main() {\n    while true {}\n    break\n}",
        &[(Code::JumpOutsideLoop, (3, 5))],
    );
}

#[test]
fn break_after_while_let() {
    codes(
        "fn f(n: i64?) {\n    while let x = n {}\n    break\n}\nfn main() {}",
        &[(Code::JumpOutsideLoop, (3, 5))],
    );
}

#[test]
fn block_value_and_type() {
    contains(
        "fn main() {\n    let a: u8 = { let w = 2\n 3 }\n}",
        &["(block (let w#0 2) 3u8)"],
    );
    codes(
        "fn main() {\n    let u = { print(1) }\n}",
        &[(Code::NoValue, (2, 13))],
    );
}

#[test]
fn block_scope_ends() {
    let printed = lowered(
        "fn main() {\n    let y = 1\n    let x = { let y = 2\n y }\n    print(y)\n    print(x)\n}",
    );
    assert!(printed.contains("(print y#0) (print x#2)"), "{printed}");
}

#[test]
fn block_joins_never() {
    clean(
        "fn f(c: bool) -> i64 {\n    let x = { if c { return 1 } else { 2 } }\n    x\n}\nfn main() {}",
    );
}

#[test]
fn block_ending_in_never() {
    // A `never` block has no value to bind, as an `if` whose branches both return has none.
    for init in ["{ return 1 }", "if c { return 1 } else { return 2 }"] {
        let src =
            format!("fn f(c: bool) -> i64 {{\n    let x: i64 = {init}\n    x\n}}\nfn main() {{}}");
        codes(&src, &[(Code::NoValue, (2, 18))]);
    }
}

#[test]
fn break_in_header() {
    // Each row: source, the keyword's position and text, the loop keyword's position and text.
    for (src, jump, lp) in [
        (
            "fn main() {\n    while { break } { }\n}",
            (2, 13, "break"),
            (2, 5, "while"),
        ),
        (
            "fn main() {\n    while { continue\n true } { }\n}",
            (2, 13, "continue"),
            (2, 5, "while"),
        ),
        (
            "fn main() {\n    while let x = { break } { }\n}",
            (2, 21, "break"),
            (2, 5, "while"),
        ),
        (
            "fn main() {\n    while let x = { continue } { }\n}",
            (2, 21, "continue"),
            (2, 5, "while"),
        ),
        (
            "fn main() {\n    for x in { break\n [1] } { }\n}",
            (2, 16, "break"),
            (2, 5, "for"),
        ),
        (
            "fn main() {\n    for x in { continue\n [1] } { }\n}",
            (2, 16, "continue"),
            (2, 5, "for"),
        ),
        // The inner loop is the one whose header holds the jump.
        (
            "fn main() {\n    while true {\n        for x in { continue\n [1] } { }\n    }\n}",
            (3, 20, "continue"),
            (3, 9, "for"),
        ),
    ] {
        let d = diagnostic(src, Code::JumpInLoopHeader, jump, &[(lp.0, lp.1)]);
        let span = d.secondary()[0].0;
        assert_eq!(&src[span.range()], lp.2, "{src}");
    }
}

#[test]
fn break_in_loop_inside_header() {
    clean("fn main() {\n    while { while true { break }\n true } { break }\n}");
}

#[test]
fn statement_after_break() {
    codes(
        "fn main() {\n    while true {\n        break\n        print(1)\n    }\n}",
        &[(Code::UnreachableCode, (4, 9))],
    );
}

#[test]
fn shadowing_makes_two_locals() {
    assert_eq!(
        lowered(
            "fn main() {\n    let x = 1\n    if true {\n        let x = true\n        print(x)\n    }\n    print(x)\n}"
        ),
        "(fn main () unit (block (let x#0 1) (if true (block (let x#1 true) (print x#1))) (print x#0)))"
    );
}

#[test]
fn lowers_compound_assign() {
    assert_eq!(
        lowered("fn main() {\n    let mut x = 1\n    x *= 3\n    print(x)\n}"),
        "(fn main () unit (block (let mut x#0 1) (= x#0 (* x#0 3)) (print x#0)))"
    );
}

#[test]
fn lowers_not_equal() {
    assert_eq!(
        lowered("fn f(a: i64) -> bool { a != 1 }\nfn main() {}"),
        "(fn f ((a#0 i64)) bool (block (!= a#0 1)))\n(fn main () unit (block))"
    );
}

#[test]
fn unknown_type_in_signature() {
    diagnostic(
        "fn f(a: str) {}\nfn main() {}",
        Code::UnknownType,
        (1, 9, "str"),
        &[],
    );
}

#[test]
fn condition_must_be_bool() {
    codes("fn main() { if 1 { } }", &[(Code::Mismatch, (1, 16))]);
}

#[test]
fn while_condition_must_be_bool() {
    codes("fn main() { while 1 { } }", &[(Code::Mismatch, (1, 19))]);
}

#[test]
fn arithmetic_needs_i64() {
    codes(
        "fn main() { print(1 + true) }",
        &[(Code::Mismatch, (1, 23))],
    );
}

#[test]
fn not_needs_bool() {
    codes("fn main() { print(!1) }", &[(Code::Mismatch, (1, 20))]);
}

#[test]
fn equality_needs_same_type() {
    codes(
        "fn main() { print(1 == true) }",
        &[(Code::CannotCompare, (1, 19))],
    );
}

#[test]
fn arity() {
    diagnostic(
        "fn f(a: i64) {}\nfn main() { f() }",
        Code::ArgumentCount,
        (2, 13, "f()"),
        &[],
    );
    diagnostic(
        "fn f(a: i64, b: i64) {}\nfn main() { f(1) }",
        Code::ArgumentCount,
        (2, 13, "f(1)"),
        &[],
    );
}

#[test]
fn argument_type() {
    codes(
        "fn f(a: i64) {}\nfn main() { f(true) }",
        &[(Code::Mismatch, (2, 15))],
    );
}

#[test]
fn print_no_argument() {
    codes("fn main() { print() }", &[(Code::PrintArgument, (1, 13))]);
}

#[test]
fn print_two_arguments() {
    codes(
        "fn main() { print(1, 2) }",
        &[(Code::PrintArgument, (1, 13))],
    );
}

#[test]
fn print_unit_argument() {
    codes(
        "fn g() {}\nfn main() { print(g()) }",
        &[(Code::PrintArgument, (2, 13))],
    );
}

#[test]
fn bind_unit() {
    codes(
        "fn g() {}\nfn main() { let x = g() }",
        &[(Code::NoValue, (2, 21))],
    );
}

#[test]
fn if_without_else_has_no_value() {
    codes(
        "fn main() {\n    let x = if true { 1 }\n}",
        &[(Code::NoValue, (2, 13))],
    );
}

#[test]
fn if_branches_differ() {
    codes(
        "fn main() {\n    let x = if true { 1 } else { false }\n}",
        &[(Code::Mismatch, (2, 34))],
    );
}

#[test]
fn body_value_type() {
    codes(
        "fn f() -> i64 {\n    true\n}\nfn main() {}",
        &[(Code::Mismatch, (2, 5))],
    );
}

#[test]
fn body_without_value() {
    let src = "fn f() -> i64 {\n    let x = 1\n}\nfn main() {}";
    codes(src, &[(Code::BodyHasNoValue, (3, 1))]);
    assert_eq!(diags(src)[0].span(), Span::new(30, 31));
}

#[test]
fn return_value_type() {
    codes(
        "fn f() -> i64 {\n    return true\n}\nfn main() {}",
        &[(Code::Mismatch, (2, 12))],
    );
}

#[test]
fn return_needs_value() {
    codes(
        "fn f() -> i64 {\n    return\n}\nfn main() {}",
        &[(Code::ReturnNeedsValue, (2, 5))],
    );
}

#[test]
fn return_value_in_unit_fn() {
    codes(
        "fn main() {\n    return 1\n}",
        &[(Code::UnexpectedReturnValue, (2, 12))],
    );
}

#[test]
fn all_paths_return_is_fine() {
    clean(
        "fn sign(n: i64) -> i64 {\n    if n < 0 { return -1 } else { return 1 }\n}\nfn main() {}",
    );
}

#[test]
fn all_paths_of_if_let_return_is_fine() {
    clean(
        "fn f(x: i64?) -> i64 {\n    if let v = x { return v } else { return 0 }\n}\nfn main() {}",
    );
}

#[test]
fn never_as_return_value() {
    codes(
        "fn f(c: bool) -> i64 {\n    return if c { return 1 } else { return 2 }\n}\nfn main() {}",
        &[(Code::NoValue, (2, 12))],
    );
}

#[test]
fn annotated_let_type() {
    codes(
        "fn main() { let x: bool = 1 }",
        &[(Code::Mismatch, (1, 27))],
    );
}

#[test]
fn annotated_let_matches() {
    clean("fn main() {\n    let x: bool = true\n    print(x)\n}");
}

#[test]
fn never_as_initializer() {
    codes(
        "fn f(c: bool) -> i64 {\n    let x = if c { return 1 } else { return 2 }\n    x\n}\nfn main() {}",
        &[(Code::NoValue, (2, 13))],
    );
}

#[test]
fn never_as_condition() {
    codes(
        "fn f(c: bool) -> i64 {\n    while if c { return 1 } else { return 2 } {}\n    1\n}\nfn main() {}",
        &[(Code::NoValue, (2, 11))],
    );
}

#[test]
fn never_as_operand() {
    codes(
        "fn f(c: bool) -> i64 {\n    1 + if c { return 1 } else { return 2 }\n}\nfn main() {}",
        &[(Code::NoValue, (2, 9))],
    );
}

/// A condition of the wrong type leaves the branches' `Never`: the statement after the `if` is
/// unreachable, and the body does not fall through to its end.
#[test]
fn failed_condition_keeps_never() {
    for (head, expected) in [
        (
            "if 1 { return 1 } else { return 2 }",
            (Code::Mismatch, (2, 8)),
        ),
        (
            "if let v = 1 { return 1 } else { return 2 }",
            (Code::UnwrapNeedsOptional, (2, 16)),
        ),
        (
            "if c { return 1 } else if 1 { return 2 } else { return 3 }",
            (Code::Mismatch, (2, 31)),
        ),
    ] {
        codes(
            &format!("fn f(c: bool) -> i64 {{\n    {head}\n    print(3)\n}}\nfn main() {{}}"),
            &[expected, (Code::UnreachableCode, (3, 5))],
        );
    }
}

#[test]
fn statement_after_return() {
    codes(
        "fn main() {\n    return\n    print(1)\n}",
        &[(Code::UnreachableCode, (3, 5))],
    );
}

#[test]
fn block_after_return_is_never() {
    codes(
        "fn f() -> i64 {\n    return 1\n    print(2)\n}\nfn main() {}",
        &[(Code::UnreachableCode, (3, 5))],
    );
}

#[test]
fn neg_needs_i64() {
    codes("fn main() { print(-true) }", &[(Code::Mismatch, (1, 20))]);
    // An unknown name is reported once: the minus adds nothing beside it.
    codes(
        "fn main() { print(-nope) }",
        &[(Code::BindingNotFound, (1, 20))],
    );
}

#[test]
fn less_needs_i64() {
    codes(
        "fn main() { print(true < false) }",
        &[(Code::Mismatch, (1, 19))],
    );
}

#[test]
fn and_needs_bool() {
    codes(
        "fn main() { print(1 && true) }",
        &[(Code::Mismatch, (1, 19))],
    );
}

#[test]
fn unit_operand() {
    codes(
        "fn g() {}\nfn main() { print(1 + g()) }",
        &[(Code::NoValue, (2, 23))],
    );
}

#[test]
fn unit_argument() {
    codes(
        "fn g() {}\nfn f(a: i64) {}\nfn main() { f(g()) }",
        &[(Code::NoValue, (3, 15))],
    );
}

#[test]
fn separate_namespaces() {
    clean("fn f() -> i64 { 1 }\nfn main() {\n    let f = 2\n    print(f)\n    print(f())\n}");
}

#[test]
fn never_branch_takes_other_type() {
    clean(
        "fn f(c: bool) -> i64 {\n    let x = if c { return 1 } else { 2 }\n    let y = if c { 3 } else { return 4 }\n    x + y\n}\nfn main() {}",
    );
}

#[test]
fn unreachable_warned_once_per_block() {
    codes(
        "fn main() {\n    return\n    print(1)\n    print(2)\n}",
        &[(Code::UnreachableCode, (3, 5))],
    );
}

#[test]
fn statements_after_return_are_checked() {
    codes(
        "fn main() {\n    return\n    print(true + 1)\n}",
        &[(Code::UnreachableCode, (3, 5)), (Code::Mismatch, (3, 11))],
    );
}

#[test]
fn assign_to_let() {
    let src = "fn main() {\n    let n = 0\n    n = 1\n}";
    let d = diagnostic(src, Code::AssignToImmutable, (3, 5, "n = 1"), &[(2, 9)]);
    assert_eq!(
        fixes(src, &d),
        [(true, vec![(2, 9, "", "mut ".to_string())])]
    );
    fix_removes_its_code(src, &d);
}

#[test]
fn never_assigned_value() {
    codes(
        "fn f(c: bool) -> i64 {\n    let mut x = 0\n    x = if c { return 1 } else { return 2 }\n    x\n}\nfn main() {}",
        &[(Code::NoValue, (3, 9))],
    );
}

#[test]
fn compound_assign_to_let() {
    diagnostic(
        "fn main() {\n    let n = 0\n    n += 1\n}",
        Code::AssignToImmutable,
        (3, 5, "n += 1"),
        &[(2, 9)],
    );
}

#[test]
fn assign_to_param() {
    diagnostic(
        "fn f(n: i64) {\n    n = 1\n}\nfn main() {}",
        Code::AssignToImmutable,
        (2, 5, "n = 1"),
        &[(1, 6)],
    );
}

#[test]
fn compound_assign_to_param() {
    diagnostic(
        "fn f(n: i64) {\n    n += 1\n}\nfn main() {}",
        Code::AssignToImmutable,
        (2, 5, "n += 1"),
        &[(1, 6)],
    );
}

#[test]
fn compound_assign_unknown_target_once() {
    codes(
        "fn main() {\n    nope += 1\n}",
        &[(Code::BindingNotFound, (2, 5))],
    );
}

#[test]
fn compound_assign_unknown_target_and_bad_value() {
    codes(
        "fn main() {\n    nope += true + 1\n}",
        &[(Code::BindingNotFound, (2, 5)), (Code::Mismatch, (2, 13))],
    );
}

#[test]
fn assign_type() {
    codes(
        "fn main() {\n    let mut x = 1\n    x = true\n}",
        &[(Code::Mismatch, (3, 9))],
    );
}

#[test]
fn unused_variable() {
    let src = "fn main() {\n    let mut count = 0\n    print(count)\n}";
    let d = diagnostic(src, Code::NeverReassigned, (2, 13, "count"), &[]);
    assert_eq!(
        fixes(src, &d),
        [(true, vec![(2, 9, "mut ", String::new())])]
    );
    fix_removes_its_code(src, &d);
}

#[test]
fn unused_variables_in_source_order() {
    // The inner scope pops first, so this fails unless warnings are sorted.
    let src = "fn main() {\n    let mut a = 0\n    if true {\n        let mut b = 0\n        print(b)\n    }\n    print(a)\n}";
    codes(
        src,
        &[
            (Code::NeverReassigned, (2, 13)),
            (Code::NeverReassigned, (4, 17)),
        ],
    );
}

#[test]
fn param_has_help_and_no_fix() {
    let src = "fn f(n: i64) { n = 1 }\nfn main() {}";
    let d = diagnostic(src, Code::AssignToImmutable, (1, 16, "n = 1"), &[(1, 6)]);
    assert!(d.help().is_some());
    assert_eq!(fixes(src, &d), []);
}

#[test]
fn for_variable_has_no_fix() {
    let src = "fn main() {\n    for i in 0..3 { i = 1 }\n}";
    let d = diagnostic(src, Code::AssignToImmutable, (2, 21, "i = 1"), &[(2, 9)]);
    assert_eq!(d.help(), None);
    assert_eq!(fixes(src, &d), []);
}

#[test]
fn two_assignments_offer_the_same_edit() {
    let src = "fn main() {\n    let n = 0\n    n = 1\n    n = 2\n    print(n)\n}";
    codes(
        src,
        &[
            (Code::AssignToImmutable, (3, 5)),
            (Code::AssignToImmutable, (4, 5)),
        ],
    );
    let ds = diags(src);
    assert_eq!(ds[0].fixes(), ds[1].fixes());
    assert_eq!(
        fixes(src, &ds[0]),
        [(true, vec![(2, 9, "", "mut ".to_string())])]
    );
}

#[test]
fn variable_reassigned_in_inner_block() {
    clean("fn main() {\n    let mut n = 0\n    if true { n = 1 }\n    print(n)\n}");
}

#[test]
fn empty_file_has_no_main() {
    let ds = diags("");
    assert_eq!(ds.len(), 1, "{ds:?}");
    assert_eq!(ds[0].code(), Code::NoMain);
    assert_eq!(at("", ds[0].span()), (1, 1));
    // The empty span at offset 0 renders without a panic.
    assert!(
        ds[0]
            .render("empty.sisu", "", &crate::source::LineIndex::new(""), false)
            .contains(Code::NoMain.as_str())
    );
}

#[test]
fn class_and_function_share_names() {
    diagnostic(
        "class A {}\nfn A() {}\nfn main() {}",
        Code::DefinedTwice,
        (2, 4, "A"),
        &[(1, 7)],
    );
}

#[test]
fn class_after_function_of_same_name() {
    diagnostic(
        "fn A() {}\nclass A {}\nfn main() {}",
        Code::DefinedTwice,
        (2, 7, "A"),
        &[(1, 4)],
    );
}

#[test]
fn class_defined_twice() {
    // The `bool` argument shows that `X` still names the first class.
    let src = "class X {\n    a: i64\n}\nclass X {\n    a: bool\n}\nfn main() { X(a: true) }";
    codes(
        src,
        &[(Code::DefinedTwice, (4, 7)), (Code::Mismatch, (7, 18))],
    );
    let secondary: Vec<_> = diags(src)[0]
        .secondary()
        .iter()
        .map(|(span, _)| at(src, *span))
        .collect();
    assert_eq!(secondary, [(1, 7)]);
}

#[test]
fn duplicate_member() {
    diagnostic(
        "class A {\n    x: i64\n    fn x(self) {}\n}\nfn main() {}",
        Code::DefinedTwice,
        (3, 8, "x"),
        &[(2, 5)],
    );
}

#[test]
fn duplicate_field() {
    diagnostic(
        "class A {\n    fn x(self) {}\n    mut x: i64\n}\nfn main() {}",
        Code::DefinedTwice,
        (3, 9, "x"),
        &[(2, 8)],
    );
}

#[test]
fn builtin_type_as_class() {
    codes("class i64 {}\nfn main() {}", &[(Code::BuiltInName, (1, 7))]);
}

#[test]
fn never_constructed_self() {
    let src = "class Node {\n    next: Node\n}\nfn main() {}";
    let d = diagnostic(src, Code::NeverConstructed, (2, 5, "next"), &[]);
    assert_eq!(fixes(src, &d), [(false, vec![(2, 15, "", "?".to_owned())])]);
    // The unsafe fix breaks the cycle, and the source still parses.
    clean(&applied(src, &d));
}

#[test]
fn never_constructed_cycle_of_two() {
    let src = "class A {\n    b: B\n}\nclass B {\n    a: A\n}\nfn main() {}";
    let d = diagnostic(src, Code::NeverConstructed, (2, 5, "b"), &[]);
    assert_eq!(fixes(src, &d), [(false, vec![(2, 9, "", "?".to_owned())])]);
    clean(&applied(src, &d));
}

#[test]
fn never_constructed_once_per_strongly_connected_component() {
    // `A`, `B` and `C` form one component of two cycles, `E` another; `D` only points in.
    codes(
        "class A {\n    b: B\n    c: C\n}\nclass B {\n    a: A\n}\nclass C {\n    a: A\n}\nclass D {\n    a: A\n}\nclass E {\n    e: E\n}\nfn main() {}",
        &[
            (Code::NeverConstructed, (2, 5)),
            (Code::NeverConstructed, (15, 5)),
        ],
    );
}

/// A class that only points into a cycle, declared first, does not hide the cycle.
#[test]
fn never_constructed_after_a_class_that_points_in() {
    diagnostic(
        "class D {\n    a: A\n}\nclass A {\n    next: A\n}\nfn main() {}",
        Code::NeverConstructed,
        (5, 5, "next"),
        &[],
    );
}

/// The field reported is the one into the cycle, not the first field of a class type.
#[test]
fn never_constructed_names_the_field_into_the_cycle() {
    diagnostic(
        "class Tag {}\nclass A {\n    tag: Tag\n    next: A\n}\nfn main() {}",
        Code::NeverConstructed,
        (4, 5, "next"),
        &[],
    );
}

/// The `?` goes right after the type, whatever spaces surround it.
#[test]
fn never_constructed_fix_follows_the_type_closely() {
    let src = "class A {\n    mut next:   A   \n}\nfn main() {}";
    let d = diagnostic(src, Code::NeverConstructed, (2, 9, "next"), &[]);
    assert_eq!(fixes(src, &d), [(false, vec![(2, 18, "", "?".to_owned())])]);
    clean(&applied(src, &d));
}

#[test]
fn constructor_missing_label() {
    let src = "class P {\n    x: i64\n}\nfn main() { P(1) }";
    let d = diagnostic(src, Code::ArgumentNeedsLabel, (4, 15, "1"), &[]);
    assert_eq!(
        fixes(src, &d),
        [(true, vec![(4, 15, "", "x: ".to_owned())])]
    );
    fix_removes_its_code(src, &d);
    // The label is the field at the argument's position.
    let src = "class P {\n    x: i64\n    y: bool\n}\nfn main() { P(x: 1, true) }";
    let d = diagnostic(src, Code::ArgumentNeedsLabel, (5, 21, "true"), &[]);
    assert_eq!(
        fixes(src, &d),
        [(true, vec![(5, 21, "", "y: ".to_owned())])]
    );
    fix_removes_its_code(src, &d);
}

#[test]
fn constructor_unknown_label() {
    diagnostic(
        "class P {\n    x: i64\n}\nfn main() { P(z: 1) }",
        Code::NoMember,
        (4, 15, "z"),
        &[],
    );
}

#[test]
fn constructor_out_of_order() {
    diagnostic(
        "class P {\n    x: i64\n    y: i64\n}\nfn main() { P(y: 1, x: 2) }",
        Code::FieldOutOfOrder,
        (5, 15, "y"),
        &[],
    );
}

#[test]
fn constructor_missing_field() {
    diagnostic(
        "class P {\n    x: i64\n    y: i64\n}\nfn main() { P(x: 1) }",
        Code::MissingField,
        (5, 13, "P(x: 1)"),
        &[],
    );
}

#[test]
fn constructor_extra_argument() {
    diagnostic(
        "class P {\n    x: i64\n}\nfn main() { P(x: 1, y: 2) }",
        Code::TooManyArguments,
        (4, 21, "y: 2"),
        &[],
    );
    diagnostic(
        "class P {\n    x: i64\n    y: i64\n}\nfn main() { P(x: 1, y: 2, 3) }",
        Code::TooManyArguments,
        (5, 27, "3"),
        &[],
    );
}

#[test]
fn constructor_field_type() {
    codes(
        "class P {\n    x: i64\n}\nfn main() { P(x: true) }",
        &[(Code::Mismatch, (4, 18))],
    );
}

#[test]
fn label_outside_constructor() {
    diagnostic(
        "fn f(a: i64) {}\nfn main() { f(a: 1) }",
        Code::LabeledArgument,
        (2, 15, "a"),
        &[],
    );
}

/// A labeled argument fails the call: it is `Error`, so a later use draws nothing.
#[test]
fn labeled_call_is_error() {
    codes(
        "fn f(a: i64) -> i64 { a }\nfn main() {\n    print(f(a: 1) + true)\n}",
        &[(Code::LabeledArgument, (3, 13))],
    );
}

/// A constructor whose own rule fails is `Error`; one with an `Error` argument keeps its
/// class, so a later use is still checked.
#[test]
fn constructor_is_error_only_when_its_rule_fails() {
    let class = "class P {\n    x: i64\n    y: i64\n}\n";
    codes(
        &format!("{class}fn main() {{\n    print(P(x: 1).x + true)\n}}"),
        &[(Code::MissingField, (6, 11))],
    );
    codes(
        &format!("{class}fn main() {{\n    print(P(x: nope, y: 1).x + true)\n}}"),
        &[(Code::BindingNotFound, (6, 16)), (Code::Mismatch, (6, 32))],
    );
}

#[test]
fn constructor_reports_its_first_mistake_and_later_expression_errors() {
    let class = "class P {\n    x: i64\n    y: i64\n}\n";
    codes(
        &format!("{class}fn main() {{ P(1, nope) }}"),
        &[
            (Code::ArgumentNeedsLabel, (5, 15)),
            (Code::BindingNotFound, (5, 18)),
        ],
    );
    codes(
        &format!("{class}fn main() {{ P(1, 2) }}"),
        &[(Code::ArgumentNeedsLabel, (5, 15))],
    );
}

#[test]
fn each_label_on_a_call_is_reported() {
    codes(
        "fn f(a: i64, b: i64) {}\nfn main() { f(a: 1, b: 2) }",
        &[
            (Code::LabeledArgument, (2, 15)),
            (Code::LabeledArgument, (2, 21)),
        ],
    );
}

#[test]
fn labels_on_unknown_calls_are_not_reported() {
    // The name may have meant the class, whose constructor takes labels.
    codes(
        "class Bucket {\n    value: i64\n}\nfn main() {\n    let t = Bukcet(value: 1)\n}",
        &[(Code::FunctionNotFound, (5, 13))],
    );
}

#[test]
fn labels_on_unknown_methods_are_reported() {
    codes(
        "class P {}\nfn main() { P().missing(x: 1 + true) }",
        &[
            (Code::NoMember, (2, 17)),
            (Code::LabeledArgument, (2, 25)),
            (Code::Mismatch, (2, 32)),
        ],
    );
    codes(
        "fn main() { nope.m(x: 1 + true) }",
        &[
            (Code::BindingNotFound, (1, 13)),
            (Code::LabeledArgument, (1, 20)),
            (Code::Mismatch, (1, 27)),
        ],
    );
}

#[test]
fn unknown_call_arguments_are_still_checked() {
    codes(
        "fn main() { nope(missing) }",
        &[
            (Code::FunctionNotFound, (1, 13)),
            (Code::BindingNotFound, (1, 18)),
        ],
    );
}

#[test]
fn unknown_target_suppresses_never_reassigned() {
    for target in ["totl = 1", "totl += 1", "totl[0] = 1"] {
        codes(
            &format!("fn main() {{\n    let mut total = 0\n    {target}\n    print(total)\n}}"),
            &[(Code::BindingNotFound, (3, 5))],
        );
    }
}

#[test]
fn unknown_target_suppresses_warnings_in_every_open_scope() {
    codes(
        "fn main() {\n    let mut a = 0\n    if true {\n        let mut b = 0\n        nope = 1\n        print(b)\n    }\n    print(a)\n}",
        &[(Code::BindingNotFound, (5, 9))],
    );
}

#[test]
fn later_binding_still_warns() {
    codes(
        "fn main() {\n    nope = 1\n    let mut b = 0\n    print(b)\n}",
        &[
            (Code::BindingNotFound, (2, 5)),
            (Code::NeverReassigned, (3, 13)),
        ],
    );
}

#[test]
fn jump_outside_a_loop_leaves_no_unreachable() {
    for jump in ["break", "continue"] {
        codes(
            &format!("fn main() {{\n    {jump}\n    print(1)\n}}"),
            &[(Code::JumpOutsideLoop, (2, 5))],
        );
    }
}

#[test]
fn jump_outside_a_loop_raises_no_return_error() {
    for jump in ["break", "continue"] {
        for tail in ["", "    print(1)\n"] {
            codes(
                &format!(
                    "fn f() -> i64 {{\n    {jump}\n{tail}}}\nfn main() {{\n    print(f())\n}}"
                ),
                &[(Code::JumpOutsideLoop, (2, 5))],
            );
        }
    }
}

#[test]
fn jump_outside_a_loop_still_checks_what_follows() {
    for jump in ["break", "continue"] {
        codes(
            &format!("fn main() {{\n    {jump}\n    let b: bool = 1\n}}"),
            &[(Code::JumpOutsideLoop, (2, 5)), (Code::Mismatch, (3, 19))],
        );
    }
}

#[test]
fn nested_invalid_jump_leaves_no_unreachable() {
    for jump in ["break", "continue"] {
        codes(
            &format!("fn main() {{\n    if true {{ {jump} }} else {{ {jump} }}\n    print(1)\n}}"),
            &[
                (Code::JumpOutsideLoop, (2, 15)),
                (Code::JumpOutsideLoop, (2, 25 + jump.len())),
            ],
        );
    }
}

#[test]
fn bad_return_value_still_warns_about_dead_code() {
    codes(
        "fn f() -> i64 {\n    return 1 + true\n    print(1)\n}\nfn main() {\n    print(f())\n}",
        &[(Code::Mismatch, (2, 16)), (Code::UnreachableCode, (3, 5))],
    );
}

#[test]
fn return_value_in_main_still_warns_about_dead_code() {
    codes(
        "fn main() {\n    return 5\n    print(1)\n}",
        &[
            (Code::UnexpectedReturnValue, (2, 12)),
            (Code::UnreachableCode, (3, 5)),
        ],
    );
}

#[test]
fn failed_branches_that_return_still_warn_about_dead_code() {
    codes(
        "fn main() {\n    if true {\n        print(missing)\n        return\n    } else {\n        return\n    }\n    print(1)\n}",
        &[
            (Code::BindingNotFound, (3, 15)),
            (Code::UnreachableCode, (8, 5)),
        ],
    );
}

/// A call to an unknown name may be a misspelled constructor, whose labels are correct: only
/// the name is reported, and the arguments are still checked.
#[test]
fn misspelled_constructor_is_one_error() {
    codes(
        "class N {\n    v: i64\n}\nfn main() { Nod(v: 1) }",
        &[(Code::FunctionNotFound, (4, 13))],
    );
    codes(
        "fn main() { nope(x: 1 + true) }",
        &[(Code::FunctionNotFound, (1, 13)), (Code::Mismatch, (1, 25))],
    );
}

#[test]
fn print_reports_labels_and_still_checks_its_arguments() {
    codes(
        "fn main() { print(z: 1, 2) }",
        &[
            (Code::PrintArgument, (1, 13)),
            (Code::LabeledArgument, (1, 19)),
        ],
    );
    codes(
        "class E {}\nfn main() { print(z: E()) }",
        &[
            (Code::LabeledArgument, (2, 19)),
            (Code::PrintArgument, (2, 22)),
        ],
    );
}

#[test]
fn unknown_field() {
    diagnostic(
        "class P {\n    x: i64\n}\nfn main() {\n    let p = P(x: 1)\n    print(p.size)\n}",
        Code::NoMember,
        (6, 13, "size"),
        &[],
    );
}

#[test]
fn field_called_as_method() {
    let src = "class P {\n    x: i64\n}\nfn main() {\n    let p = P(x: 1)\n    print(p.x())\n}";
    let d = diagnostic(src, Code::FieldNotMethod, (6, 13, "x"), &[]);
    assert!(d.fixes().is_empty(), "{d:?}");
}

#[test]
fn method_read_as_field() {
    let src = "class P {\n    fn m(self) -> i64 { 1 }\n}\nfn main() {\n    let p = P()\n    print(p.m)\n}";
    let d = diagnostic(src, Code::MethodNotField, (6, 13, "m"), &[]);
    assert_eq!(fixes(src, &d), [(true, vec![(6, 14, "", "()".to_owned())])]);
    fix_removes_its_code(src, &d);
}

/// A method that takes arguments needs more than `()`, so the fix is unsafe.
#[test]
fn method_with_arguments_read_as_field() {
    let src = "class P {\n    fn m(self, a: i64) -> i64 { a }\n}\nfn main() {\n    let p = P()\n    print(p.m)\n}";
    let d = diagnostic(src, Code::MethodNotField, (6, 13, "m"), &[]);
    assert_eq!(
        fixes(src, &d),
        [(false, vec![(6, 14, "", "()".to_owned())])]
    );
    applied(src, &d);
}

/// `()` would make an assignment target a call, which is no place. Each row is the
/// statement, in a body where `p` is a `P` and `xs` an `[i64]`.
#[test]
fn method_as_assignment_target_has_no_fix() {
    for (stmt, at) in [
        ("p.m = 1", (6, 7, "m")),
        ("p.m += 1", (6, 7, "m")),
        ("p.m[0] = 1", (6, 7, "m")),
        ("xs.len = 1", (6, 8, "len")),
        ("xs.len += 1", (6, 8, "len")),
    ] {
        let src = format!(
            "class P {{\n    mut v: i64\n    fn m(self) -> P {{ self }}\n}}\nfn f(p: P, xs: [i64]) {{\n    {stmt}\n}}\nfn main() {{}}"
        );
        let d = diagnostic(&src, Code::MethodNotField, at, &[]);
        assert!(d.fixes().is_empty(), "{stmt}: {d:?}");
    }
}

/// A method read on the way to a field that is assigned is a value, so `()` fits there.
#[test]
fn method_in_an_assignment_target_chain_has_a_fix() {
    let src = "class P {\n    mut v: i64\n    fn m(self) -> P {\n        self\n    }\n}\nfn f(p: P) {\n    p.m.v = 1\n}\nfn main() {}";
    let d = diagnostic(src, Code::MethodNotField, (8, 7, "m"), &[]);
    assert_eq!(fixes(src, &d), [(true, vec![(8, 8, "", "()".to_owned())])]);
    fix_removes_its_code(src, &d);
}

#[test]
fn unknown_method() {
    diagnostic(
        "class P {\n    fn m(self) -> i64 { 1 }\n}\nfn main() {\n    let p = P()\n    print(p.nope())\n}",
        Code::NoMember,
        (6, 13, "nope"),
        &[],
    );
}

#[test]
fn method_through_class_name() {
    codes(
        "class Tree {\n    fn sum(self) -> i64 { 0 }\n}\nfn main() {\n    print(Tree.sum(Tree()))\n}",
        &[(Code::TypeAsValue, (5, 11))],
    );
}

#[test]
fn unknown_field_type_poisons() {
    codes(
        "class T {\n    mut x: Nope\n}\nfn main() { print(T(x: 1).x + true) }",
        &[(Code::UnknownType, (2, 12))],
    );
}

#[test]
fn method_arity() {
    diagnostic(
        "class P {\n    fn m(self, a: i64) {}\n}\nfn main() { P().m() }",
        Code::ArgumentCount,
        (4, 13, "P().m()"),
        &[],
    );
}

#[test]
fn field_on_i64() {
    codes(
        "fn main() {\n    let x = 1\n    print(x.f)\n}",
        &[(Code::NoMember, (3, 13))],
    );
}

#[test]
fn method_on_i64() {
    codes(
        "fn main() {\n    let x = 1\n    print(x.m())\n}",
        &[(Code::NoMember, (3, 13))],
    );
}

#[test]
fn self_outside_method() {
    codes(
        "fn main() { print(self) }",
        &[(Code::SelfOutsideMethod, (1, 19))],
    );
}

#[test]
fn bare_name_is_not_a_field() {
    codes(
        "class P {\n    x: i64\n    fn m(self) -> i64 { x }\n}\nfn main() {}",
        &[(Code::BindingNotFound, (3, 25))],
    );
}

#[test]
fn assign_bare_field() {
    let src = "class P {\n    x: i64\n}\nfn main() {\n    let p = P(x: 1)\n    p.x = 2\n}";
    let d = diagnostic(src, Code::AssignToImmutable, (6, 5, "p.x = 2"), &[(2, 5)]);
    assert_eq!(
        fixes(src, &d),
        [(true, vec![(2, 5, "", "mut ".to_string())])]
    );
    fix_removes_its_code(src, &d);
}

#[test]
fn print_class() {
    codes(
        "class E {}\nfn main() { print(E()) }",
        &[(Code::PrintArgument, (2, 19))],
    );
}

#[test]
fn poisoned_receiver_checks_args() {
    codes(
        "fn main() { nope.m(1 + true) }",
        &[(Code::BindingNotFound, (1, 13)), (Code::Mismatch, (1, 24))],
    );
}

#[test]
fn lowers_methods() {
    assert_eq!(
        lowered(
            "class C {\n    mut n: i64\n    fn bump(self, by: i64) {\n        self.n += by\n    }\n}\nfn main() {\n    let c = C(n: 1)\n    c.bump(2)\n    print(c.n)\n}"
        ),
        "(class C (mut n i64))\n\
         (fn C.bump ((self#0 C) (by#1 i64)) unit (block (block (= (. self#0 n) (+ (. self#0 n) by#1)))))\n\
         (fn main () unit (block (let c#0 (new C 1)) (call C.bump c#0 2) (print (. c#0 n))))"
    );
}

#[test]
fn lowers_field_assignments() {
    assert_eq!(
        lowered(
            "class B {\n    mut f: i64\n}\nclass A {\n    mut b: B\n}\nfn mk(a: A) -> A { a }\nfn main() {\n    let a = A(b: B(f: 1))\n    a.b.f = 2\n    mk(a).b = B(f: 3)\n}"
        ),
        "(class B (mut f i64))\n\
         (class A (mut b B))\n\
         (fn mk ((a#0 A)) A (block a#0))\n\
         (fn main () unit (block (let a#0 (new A (new B 1))) (= (. (. a#0 b) f) 2) (= (. (call mk a#0) b) (new B 3))))"
    );
}

#[test]
fn assign_field_type() {
    codes(
        "class P {\n    mut x: i64\n}\nfn main() {\n    let p = P(x: 1)\n    p.x = true\n}",
        &[(Code::Mismatch, (6, 11))],
    );
}

#[test]
fn never_as_print_argument() {
    diagnostic(
        "fn f(c: bool) -> i64 {\n    print(if c { return 1 } else { return 2 })\n    1\n}\nfn main() {}",
        Code::PrintNever,
        (2, 11, "if c { return 1 } else { return 2 }"),
        &[],
    );
}

/// The printed `g` function, once `g`'s source sits between the spill tests' shared classes
/// and functions and an empty `main`.
fn printed_g(g: &str) -> String {
    let src = format!(
        "class B {{}}\nclass P {{\n    mut b: B\n    n: i64\n}}\nfn make() -> B {{ B() }}\nfn f(b: B, n: i64) {{}}\n{g}\nfn main() {{}}"
    );
    lowered(&src)
        .lines()
        .find(|line| line.starts_with("(fn g "))
        .expect("`g` is lowered")
        .to_owned()
}

#[test]
fn no_spill_for_an_assignment_in_a_later_operand() {
    // An assignment is not an exit, so `make()` is not spilled around the `if`.
    assert_eq!(
        printed_g(
            "fn g(c: bool, p: P) {\n    f(make(), if c {\n        p.b = make()\n        1\n    } else {\n        2\n    })\n}"
        ),
        "(fn g ((c#0 bool) (p#1 P)) unit (block (call f (call make) (if c#0 (block (= (. p#1 b) (call make)) 1) (block 2)))))"
    );
}

#[test]
fn spills_call_argument() {
    assert_eq!(
        printed_g("fn g(c: bool) -> i64 {\n    f(make(), if c { return 0 } else { 1 })\n    2\n}"),
        "(fn g ((c#0 bool)) i64 (block (block (let #1 (call make)) (let #2 (if c#0 (block (return 0)) (block 1))) (call f #1 #2)) 2))"
    );
}

#[test]
fn spills_keep_source_order() {
    assert_eq!(
        printed_g(
            "fn one() -> i64 { 1 }\nfn h(a: i64, b: B, n: i64) {}\nfn g(c: bool) -> i64 {\n    h(one(), make(), if c { return 0 } else { 1 })\n    2\n}"
        ),
        "(fn g ((c#0 bool)) i64 (block (block (let #1 (call one)) (let #2 (call make)) (let #3 (if c#0 (block (return 0)) (block 1))) (call h #1 #2 #3)) 2))"
    );
}

#[test]
fn spills_constructor_argument() {
    assert_eq!(
        printed_g(
            "fn g(c: bool) -> P {\n    P(b: make(), n: if c { return P(b: B(), n: 0) } else { 1 })\n}"
        ),
        "(fn g ((c#0 bool)) P (block (block (let #1 (call make)) (let #2 (if c#0 (block (return (new P (new B) 0))) (block 1))) (new P #1 #2))))"
    );
}

#[test]
fn spills_field_assignment_base() {
    assert_eq!(
        printed_g(
            "fn g(c: bool, p: P) {\n    while true {\n        p.b = if c { break } else { make() }\n    }\n}"
        ),
        "(fn g ((c#0 bool) (p#1 P)) unit (block (loop (block (if true (block (block (let #2 p#1) (let #3 (if c#0 (block (break)) (block (call make)))) (= (. #2 b) #3))) (block (break)))))))"
    );
}

#[test]
fn no_spill_without_exit() {
    assert_eq!(
        printed_g("fn g() {\n    f(make(), 1)\n}"),
        "(fn g () unit (block (call f (call make) 1)))"
    );
}

#[test]
fn no_spill_without_earlier_reference() {
    assert_eq!(
        printed_g("fn g(c: bool) -> i64 {\n    f(if c { return 0 } else { make() }, 1)\n    2\n}"),
        "(fn g ((c#0 bool)) i64 (block (call f (if c#0 (block (return 0)) (block (call make))) 1) 2))"
    );
}

#[test]
fn nested_loop_break_still_spills() {
    assert_eq!(
        printed_g("fn g() {\n    f(make(), if true { while true { break }\n 1 } else { 1 })\n}"),
        "(fn g () unit (block (block (let #0 (call make)) (let #1 (if true (block (loop (block (if true (block (break)) (block (break))))) 1) (block 1))) (call f #0 #1))))"
    );
}

#[test]
fn spills_method_receiver() {
    assert_eq!(
        printed_g(
            "class Q {\n    fn m(self, n: i64) {}\n}\nfn q() -> Q { Q() }\nfn g(c: bool) -> i64 {\n    q().m(if c { return 0 } else { 1 })\n    2\n}"
        ),
        "(fn g ((c#0 bool)) i64 (block (block (let #1 (call q)) (let #2 (if c#0 (block (return 0)) (block 1))) (call Q.m #1 #2)) 2))"
    );
}

/// `a.f op= e` binds `a` as the assignment's base before `e` runs. The parameter `a` itself is
/// not bound first: `e` cannot change it.
#[test]
fn spills_compound_field_assignment_base() {
    assert_eq!(
        printed_g(
            "class Q {\n    mut k: i64\n}\nfn g(c: bool, q: Q) {\n    while true {\n        q.k += if c { break } else { 1 }\n    }\n}"
        ),
        "(fn g ((c#0 bool) (q#1 Q)) unit (block (loop (block (if true (block (block (block (let #2 q#1) (let #3 (+ (. q#1 k) (if c#0 (block (break)) (block 1)))) (= (. #2 k) #3)))) (block (break)))))))"
    );
}

#[test]
fn spills_every_operand_through_the_last_exit() {
    assert_eq!(
        printed_g(
            "fn one() -> i64 { 1 }\nfn h(b: B, x: i64, y: i64, z: i64) {}\nfn g(c: bool) -> i64 {\n    h(make(), if c { return 0 } else { 1 }, if c { return 1 } else { 2 }, one())\n    2\n}"
        ),
        "(fn g ((c#0 bool)) i64 (block (block (let #1 (call make)) (let #2 (if c#0 (block (return 0)) (block 1))) (let #3 (if c#0 (block (return 1)) (block 2))) (call h #1 #2 #3 (call one))) 2))"
    );
}

#[test]
fn spills_for_an_exit_in_a_nested_assignment() {
    assert_eq!(
        printed_g(
            "fn g(c: bool) -> i64 {\n    let mut x = 0\n    f(make(), if true {\n        x = if c { return 0 } else { 1 }\n        x\n    } else { 0 })\n    2\n}"
        ),
        "(fn g ((c#0 bool)) i64 (block (let mut x#1 0) (block (let #2 (call make)) (let #3 (if true (block (= x#1 (if c#0 (block (return 0)) (block 1))) x#1) (block 0))) (call f #2 #3)) 2))"
    );
}

#[test]
fn no_spill_with_only_scalars_before_the_exit() {
    assert_eq!(
        printed_g(
            "fn one() -> i64 { 1 }\nfn h2(a: i64, n: i64) {}\nfn g(c: bool) -> i64 {\n    h2(one(), if c { return 0 } else { 1 })\n    2\n}"
        ),
        "(fn g ((c#0 bool)) i64 (block (call h2 (call one) (if c#0 (block (return 0)) (block 1))) 2))"
    );
}

/// A class-optional operand holds a reference, and an `if let` can exit.
#[test]
fn spills_class_optional_before_exit_in_if_let() {
    assert_eq!(
        printed_g(
            "fn maybe() -> B? { None }\nfn h(b: B?, n: i64) {}\nfn g(c: bool, n: B?) -> i64 {\n    h(maybe(), if let x = n { return 0 } else { 1 })\n    2\n}"
        ),
        "(fn g ((c#0 bool) (n#1 B?)) i64 (block (block (let #3 (call maybe)) (let #4 (if-some x#2 n#1 (block (return 0)) (block 1))) (call h #3 #4)) 2))"
    );
}

/// The class the optionals tests share: its `next` field is optional.
const T: &str = "class T {\n    v: i64\n    next: T?\n}\n";

/// Asserts that `f`, lowered after `prefix` and before an empty `main`, prints `part`.
fn f_contains(prefix: &str, f: &str, part: &str) {
    let printed = lowered(&format!("{prefix}{f}\nfn main() {{}}"));
    assert!(printed.contains(part), "{printed}");
}

#[test]
fn none_needs_a_type() {
    diagnostic(
        "fn main() { let x = None }",
        Code::CannotInfer,
        (1, 21, "None"),
        &[],
    );
}

#[test]
fn if_of_two_nones() {
    codes(
        "class T {}\nfn main() {\n    let x = if true { None } else { None }\n}",
        &[(Code::CannotInfer, (3, 23))],
    );
}

#[test]
fn none_for_non_optional() {
    codes(
        "fn main() { let x: i64 = None }",
        &[(Code::Mismatch, (1, 26))],
    );
}

#[test]
fn dot_on_optional() {
    let src = "class T {\n    v: i64\n}\nfn f(node: T?) -> i64? { node.v }\nfn main() {}";
    let d = diagnostic(src, Code::MayBeNone, (4, 26, "node"), &[]);
    assert_eq!(
        fixes(src, &d),
        [(false, vec![(4, 30, ".", "?.".to_owned())])]
    );
    clean(&applied(src, &d));
}

/// The fix replaces the gap between any receiver and its member, for a field and a method.
#[test]
fn may_be_none_fix_follows_any_receiver() {
    let src = "class N {\n    next: N?\n    fn m(self) -> i64 { 1 }\n}\nfn g() -> N? { None }\nfn f(n: N) {\n    let a = n.next.m()\n    let b = g() // here\n        .next\n}\nfn main() {}";
    let ds = diags(src);
    let found: Vec<_> = ds.iter().map(|d| (d.code(), fixes(src, d))).collect();
    assert_eq!(
        found,
        [
            (
                Code::MayBeNone,
                vec![(false, vec![(7, 19, ".", "?.".to_owned())])]
            ),
            (
                Code::MayBeNone,
                vec![(false, vec![(8, 16, " // here\n        .", "?.".to_owned())])]
            ),
        ]
    );
    for d in &ds {
        applied(src, d);
    }
}

#[test]
fn optional_self_field_is_reported_at_its_path() {
    diagnostic(
        "class N {\n    next: N?\n    v: i64\n    fn m(self) -> i64 { self.next.v }\n}\nfn main() {}",
        Code::MayBeNone,
        (4, 25, "self.next"),
        &[],
    );
}

/// `?.` would make an assignment target go through `?.`, which does not parse. Each row is a
/// statement in a body where `m` is an `N?`.
#[test]
fn may_be_none_in_an_assignment_target_has_no_fix() {
    for stmt in ["m.v = 1", "m.v += 1", "m.get().v = 1", "m.o.v = 1"] {
        let src = format!(
            "class N {{\n    mut v: i64\n    o: N?\n    fn get(self) -> N {{\n        self\n    }}\n}}\nfn f(m: N?) {{\n    {stmt}\n}}\nfn main() {{}}"
        );
        let d = diagnostic(&src, Code::MayBeNone, (9, 5, "m"), &[]);
        assert!(d.fixes().is_empty(), "{stmt}: {d:?}");
    }
}

/// Inside an assignment target, an argument or an index is an ordinary read, so its own
/// `n.next` keeps the `?.` unsafe fix. Each row is a statement where `n` is an `N`; the last is an
/// argument inside an index.
#[test]
fn may_be_none_in_an_operand_of_an_assignment_target_keeps_its_fix() {
    for stmt in [
        "n.at(n.next.v).v = 1",
        "xs[n.next.v].v = 1",
        "g(n.next.v).v = 1",
        "xs[n.at(n.next.v).v].v = 1",
    ] {
        let src = format!(
            "class N {{\n    mut v: i64\n    next: N?\n    fn at(self, i: i64) -> N {{\n        self\n    }}\n}}\nfn g(i: i64) -> N {{ g(i) }}\nfn f(n: N, xs: [N]) {{\n    {stmt}\n}}\nfn main() {{}}"
        );
        let col = 5 + stmt.find("n.next").expect("each row reads `n.next`");
        let d = diagnostic(&src, Code::MayBeNone, (10, col, "n.next"), &[]);
        assert_eq!(
            fixes(&src, &d),
            [(false, vec![(10, col + 6, ".", "?.".to_owned())])],
            "{stmt}"
        );
        let left: Vec<_> = diags(&applied(&src, &d))
            .iter()
            .map(Diagnostic::code)
            .collect();
        assert!(!left.contains(&Code::MayBeNone), "{stmt}: {left:?}");
    }
}

/// An array method's arguments are reads, not part of the target. The target itself is
/// ill-typed, so other codes appear; only `MayBeNone` is asserted.
#[test]
fn may_be_none_in_an_array_method_argument_of_a_target_keeps_its_fix() {
    for stmt in [
        "nums.contains(n.next.v).v = 1",
        "nums.len(n.next.v).v = 1",
        "nums.nope(n.next.v).v = 1",
    ] {
        let src = format!(
            "class N {{\n    mut v: i64\n    next: N?\n}}\nfn f(n: N, nums: [i64]) {{\n    {stmt}\n}}\nfn main() {{}}"
        );
        let col = 5 + stmt.find("n.next").expect("each row reads `n.next`");
        let ds = diags(&src);
        let nones: Vec<_> = ds.iter().filter(|d| d.code() == Code::MayBeNone).collect();
        assert_eq!(nones.len(), 1, "{stmt}: {ds:?}");
        let d = nones[0];
        assert_eq!(at(&src, d.span()), (6, col), "{stmt}");
        assert_eq!(
            fixes(&src, d),
            [(false, vec![(6, col + 6, ".", "?.".to_owned())])],
            "{stmt}"
        );
        let left: Vec<_> = diags(&applied(&src, d))
            .iter()
            .map(Diagnostic::code)
            .collect();
        assert!(!left.contains(&Code::MayBeNone), "{stmt}: {left:?}");
    }
}

/// The receiver `n.next` is the target's own chain and gets no fix; the argument's does.
#[test]
fn may_be_none_in_a_target_chain_and_in_its_argument() {
    let src = "class N {\n    mut v: i64\n    next: N?\n    fn at(self, i: i64) -> N {\n        self\n    }\n}\nfn f(n: N) {\n    n.next.at(n.next.v).v = 1\n}\nfn main() {}";
    let found: Vec<_> = diags(src)
        .iter()
        .map(|d| (d.code(), at(src, d.span()), fixes(src, d)))
        .collect();
    assert_eq!(
        found,
        [
            (Code::MayBeNone, (9, 5), vec![]),
            (
                Code::MayBeNone,
                (9, 15),
                vec![(false, vec![(9, 21, ".", "?.".to_owned())])]
            ),
        ]
    );
}

/// The target's chain, where `n.next` gets no fix, ends with its statement: a read after it
/// keeps the unsafe fix.
#[test]
fn may_be_none_after_an_assignment_target_keeps_its_fix() {
    let src = "class N {\n    mut v: i64\n    next: N?\n}\nfn f(n: N, k: N) {\n    n.next.next.v = 1\n    print(k.next.v)\n}\nfn main() {}";
    let found: Vec<_> = diags(src)
        .iter()
        .map(|d| (d.code(), at(src, d.span()), fixes(src, d)))
        .collect();
    assert_eq!(
        found,
        [
            (Code::MayBeNone, (6, 5), vec![]),
            (
                Code::MayBeNone,
                (7, 11),
                vec![(false, vec![(7, 17, ".", "?.".to_owned())])]
            ),
        ]
    );
}

#[test]
fn if_let_needs_optional() {
    codes(
        "fn main() {\n    if let x = 5 {}\n}",
        &[(Code::UnwrapNeedsOptional, (2, 16))],
    );
}

#[test]
fn while_let_needs_optional() {
    codes(
        "fn main() {\n    while let x = 5 {}\n}",
        &[(Code::UnwrapNeedsOptional, (2, 19))],
    );
}

#[test]
fn failed_if_let_binding_poisons() {
    codes(
        "fn main() {\n    if let x = 5 {\n        print(x + true)\n    }\n}",
        &[(Code::UnwrapNeedsOptional, (2, 16))],
    );
}

#[test]
fn if_let_scope() {
    codes(
        &format!(
            "{T}fn f(n: T?) {{\n    if let x = n {{}} else {{ let y = x }}\n}}\nfn main() {{}}"
        ),
        &[(Code::BindingNotFound, (6, 36))],
    );
}

#[test]
fn optional_breaks_the_cycle() {
    clean(&format!("{T}fn main() {{}}"));
}

#[test]
fn none_from_every_source() {
    clean(&format!(
        "{T}fn take(t: T?) {{}}\nfn give() -> T? {{ None }}\nfn main() {{\n    let a: T? = None\n    let b = T(v: 1, next: None)\n    take(None)\n    let mut c: T? = if true {{ None }} else if false {{ None }} else {{ None }}\n    c = None\n    return\n}}\nfn back(c: bool) -> T? {{\n    if c {{ return None }}\n    if let x = give() {{ None }} else {{ None }}\n}}"
    ));
}

#[test]
fn assign_if_let_binding() {
    diagnostic(
        &format!(
            "{T}fn f(n: T?) {{\n    if let x = n {{\n        x = T(v: 1, next: None)\n    }}\n}}\nfn main() {{}}"
        ),
        Code::AssignToImmutable,
        (7, 9, "x = T(v: 1, next: None)"),
        &[(6, 12)],
    );
}

#[test]
fn assign_while_let_binding() {
    diagnostic(
        &format!(
            "{T}fn f(n: T?) {{\n    while let x = n {{\n        x = T(v: 1, next: None)\n    }}\n}}\nfn main() {{}}"
        ),
        Code::AssignToImmutable,
        (7, 9, "x = T(v: 1, next: None)"),
        &[(6, 15)],
    );
}

#[test]
fn print_optional() {
    codes(
        &format!("{T}fn f(x: T?) {{ print(x) }}\nfn main() {{}}"),
        &[(Code::PrintArgument, (5, 15))],
    );
    codes(
        "fn f(x: i64?) { print(x) }\nfn main() {}",
        &[(Code::PrintArgument, (1, 17))],
    );
}

/// A `None` that its rule rejects fails that rule: what the rule makes is `Error`, so a
/// later use draws nothing.
#[test]
fn rejected_none_reports_once() {
    let cases = [
        (
            "fn main() {\n    let x = !None\n    print(x + 1)\n}",
            (2, 14),
        ),
        (
            "fn main() {\n    let x = if None { 1 } else { 2 }\n    print(x && true)\n}",
            (2, 16),
        ),
        (
            "fn f(x: i64) -> i64 { x }\nfn main() {\n    let r = f(None)\n    print(r && true)\n}",
            (3, 15),
        ),
        (
            "fn main() {\n    let y = None + 1\n    print(y && true)\n}",
            (2, 13),
        ),
    ];
    for (src, pos) in cases {
        codes(src, &[(Code::Mismatch, pos)]);
    }
    // Each `None` is its own mistake; the `if` that holds them fails the rule too.
    let nested = [
        (
            "fn f(c: bool) {\n    let x = !(if c { None } else { None })\n    print(x + 1)\n}"
                .to_owned(),
            [22, 36].as_slice(),
            2,
        ),
        (
            "fn f(c: bool) {\n    let x = !(if c { if c { None } else { None } } else { None })\n    print(x + 1)\n}"
                .to_owned(),
            &[29, 43, 59],
            2,
        ),
        (
            format!("{T}fn f(n: T?) {{\n    let x = !(if let m = n {{ None }} else {{ None }})\n    print(x + 1)\n}}"),
            &[30, 44],
            6,
        ),
    ];
    for (src, cols, line) in nested {
        let expected: Vec<_> = cols
            .iter()
            .map(|&col| (Code::Mismatch, (line, col)))
            .collect();
        codes(&format!("{src}\nfn main() {{}}"), &expected);
    }
    // A rejected condition or scrutinee keeps the branches' mismatch.
    codes(
        "fn main() {\n    let x = !(if 1 { None } else { None })\n    print(x + 1)\n}",
        &[
            (Code::Mismatch, (2, 18)),
            (Code::Mismatch, (2, 22)),
            (Code::Mismatch, (2, 36)),
        ],
    );
    codes(
        "fn main() {\n    let x = !(if let y = 5 { None } else { None })\n    print(x + 1)\n}",
        &[
            (Code::UnwrapNeedsOptional, (2, 26)),
            (Code::Mismatch, (2, 30)),
            (Code::Mismatch, (2, 44)),
        ],
    );
}

/// An `if let` whose value is not optional fails: it is `Error`, so a later use draws
/// nothing.
#[test]
fn rejected_if_let_scrutinee_reports_once() {
    codes(
        "fn main() {\n    let x = if let y = 5 { 1 } else { 2 }\n    print(x && true)\n}",
        &[(Code::UnwrapNeedsOptional, (2, 24))],
    );
}

/// Branches that are `Error` by their own mistakes did not reject the type the `!` passed
/// down, so the `!` is still `bool`, and a later use is checked against it.
#[test]
fn error_branches_without_mismatch_keep_the_rule_type() {
    codes(
        "fn f(c: bool) {\n    let x = !(if c { nope } else { nope })\n    print(x + 1)\n}\nfn main() {}",
        &[
            (Code::BindingNotFound, (2, 22)),
            (Code::BindingNotFound, (2, 36)),
            (Code::Mismatch, (3, 11)),
        ],
    );
}

/// A body whose tail failed keeps its type, so a `T?` function still wraps its `T` value
/// and nothing more is reported.
#[test]
fn failed_tail_of_an_optional_body_reports_once() {
    codes(
        "fn f(c: bool) -> i64? {\n    let a = 1\n    if c { nope } else { a }\n}\nfn main() {}",
        &[(Code::BindingNotFound, (3, 12))],
    );
}

/// With a type expected, a join that fails is reported at the branch that fits it neither
/// as is nor as its payload, in either order, from a body, a `return` or a `let`.
#[test]
fn failed_join_blames_the_branch_that_does_not_fit() {
    let sites = [
        ("fn f(c: bool) -> T? {\n    ", 5),
        ("fn f(c: bool) -> T? {\n    return ", 12),
        ("fn f(c: bool) {\n    let x: T? = ", 17),
    ];
    let values = [
        ("if c { 1 } else { None }", 7),
        ("if c { None } else { 1 }", 21),
    ];
    for (site, start) in sites {
        for (value, offset) in values {
            codes(
                &format!("{T}{site}{value}\n}}\nfn main() {{}}"),
                &[(Code::Mismatch, (6, start + offset))],
            );
        }
    }
}

/// A `unit` or `never` branch types no `None` beside it.
#[test]
fn none_beside_unit_or_never_needs_a_type() {
    for (value, col) in [
        ("if c { g() } else { None }", 25),
        ("if c { return } else { None }", 28),
    ] {
        codes(
            &format!(
                "fn g() {{}}\nfn f(c: bool) {{\n    {value}\n    print(1)\n}}\nfn main() {{}}"
            ),
            &[(Code::CannotInfer, (3, col))],
        );
    }
}

/// Every position that expects a `T?` wraps a `T`.
#[test]
fn wraps_where_optional_expected() {
    let src = "fn g(x: T?) {}\nfn f(t: T) -> T? {\n    let a: T? = t\n    let mut b: T? = None\n    b = t\n    g(t)\n    let c = T(v: 1, next: t)\n    return t\n}";
    for part in [
        "(let a#1 (wrap t#0))",
        "(= b#2 (wrap t#0))",
        "(call g (wrap t#0))",
        "(new T 1 (wrap t#0))",
        "(return (wrap t#0))",
    ] {
        f_contains(T, src, part);
    }
}

#[test]
fn lowers_else_if_let() {
    f_contains(
        T,
        "fn f(a: T?, b: T?) -> i64 {\n    if let x = a { x.v } else if let y = b { y.v } else { 0 }\n}",
        "(if-some x#2 a#0 (block (. x#2 v)) (block (if-some y#3 b#1 (block (. y#3 v)) (block 0))))",
    );
}

#[test]
fn if_let_shadows() {
    clean(&format!(
        "{T}fn f(node: T?) -> i64 {{\n    if let node = node {{ node.v }} else {{ 0 }}\n}}\nfn main() {{}}"
    ));
}

/// The `?` is not part of the unknown name, and the `None` it would type draws nothing.
#[test]
fn unknown_optional_type_underlines_the_name() {
    let src = "fn main() { let x: Foo? = None }";
    let ds = diags(src);
    assert_eq!(ds.len(), 1, "{ds:?}");
    assert_eq!(ds[0].code(), Code::UnknownType);
    assert_eq!(&src[ds[0].span().range()], "Foo");
}

#[test]
fn wraps_a_value_where_optional_expected() {
    f_contains(
        T,
        "fn f() -> T? { T(v: 1, next: None) }",
        "(block (wrap (new T 1 None)))",
    );
}

#[test]
fn joins_if_branches() {
    f_contains(
        T,
        "fn f(c: bool, t: T) -> T? {\n    let x = if c { None } else { t }\n    x\n}",
        "(let x#2 (if c#0 (block None) (block (wrap t#1))))",
    );
}

/// The spec's `if c { t } else { None }`: the value comes first, and its branch is wrapped.
#[test]
fn joins_a_value_then_none() {
    f_contains(
        T,
        "fn f(c: bool, t: T) -> T? {\n    let x = if c { t } else { None }\n    x\n}",
        "(let x#2 (if c#0 (block (wrap t#1)) (block None)))",
    );
}

#[test]
fn wraps_an_if_let_binding_beside_none() {
    f_contains(
        T,
        "fn f(n: T?) {\n    let y = if let x = n { x } else { None }\n}",
        "(let y#2 (if-some x#1 n#0 (block (wrap x#1)) (block None)))",
    );
}

/// A `T` meets a `T?` that is not a bare `None`: the `T` branch is wrapped.
#[test]
fn wraps_a_value_beside_an_optional_call() {
    f_contains(
        T,
        "fn give() -> T? { None }\nfn f(c: bool, t: T) {\n    let x = if c { t } else { give() }\n}",
        "(let x#2 (if c#0 (block (wrap t#1)) (block (call give))))",
    );
}

#[test]
fn lowers_if_let() {
    f_contains(
        T,
        "fn f(n: T?) -> i64 {\n    if let x = n { x.v } else { 0 }\n}",
        "(if-some x#1 n#0 (block (. x#1 v)) (block 0))",
    );
}

#[test]
fn lowers_while_let() {
    f_contains(
        T,
        "fn f(l: T?) -> i64 {\n    let mut n = 0\n    let mut node = l\n    while let x = node {\n        n += 1\n        node = x.next\n    }\n    n\n}",
        "(loop (block (if-some x#3 node#2 (block (= n#1 (+ n#1 1)) (= node#2 (. x#3 next))) (block (break)))))",
    );
}

#[test]
fn safe_call_on_non_optional() {
    let src = "class T {\n    v: i64\n}\nfn f(t: T) -> i64? { t?.v }\nfn main() {}";
    let d = diagnostic(src, Code::SafeAccessNotOptional, (4, 22, "t"), &[]);
    assert_eq!(
        fixes(src, &d),
        [(true, vec![(4, 23, "?.", ".".to_owned())])]
    );
}

/// Each row: the expression, and the text between the receiver and the member that the fix
/// replaces. The rows cover each shape of receiver, a member that is a call, a chain, and a
/// gap with a line break and a comment.
#[test]
fn safe_call_fix_replaces_the_gap_for_each_receiver() {
    let rows = [
        ("mk()?.v", "?."),
        ("mk()?.get()", "?."),
        ("p?.o?.v", "?."),
        ("a[0]?.v", "?."),
        ("(p)?.v", "?."),
        ("p\n        ?.v", "\n        ?."),
        ("p // note\n        ?.v", " // note\n        ?."),
    ];
    for (expr, gap) in rows {
        let src = format!(
            "class T {{\n    v: i64\n    o: T?\n    fn get(self) -> i64 {{ self.v }}\n}}\nfn mk() -> T {{\n    T(v: 1, o: None)\n}}\nfn f(p: T, a: [T]) {{\n    let r = {expr}\n}}\nfn main() {{}}"
        );
        let ds = diags(&src);
        let [d] = ds.as_slice() else {
            panic!("{src}: {ds:?}")
        };
        assert_eq!(d.code(), Code::SafeAccessNotOptional, "{src}");
        let [fix] = d.fixes() else {
            panic!("{src}: one fix")
        };
        assert!(fix.safe(), "{src}");
        let [edit] = fix.edits() else {
            panic!("{src}: {:?}", fix.edits())
        };
        assert_eq!(edit.replacement(), ".", "{src}");
        assert_eq!(&src[edit.span().range()], gap, "{src}");
        // The edited source parses, and has no `?.` error left.
        let result = applied(&src, d);
        let left: Vec<_> = diags(&result).iter().map(Diagnostic::code).collect();
        assert!(!left.contains(&d.code()), "{result}: {left:?}");
    }
}

/// In `a?.b.c`, the `.c` applies to a `B?`.
#[test]
fn safe_chain_needs_each_step() {
    let src = "class A {\n    b: B\n}\nclass B {\n    c: i64\n}\nfn f(a: A?) -> i64? { a?.b.c }\nfn main() {}";
    let d = diagnostic(src, Code::MayBeNone, (7, 23, "a?.b"), &[]);
    assert_eq!(
        fixes(src, &d),
        [(false, vec![(7, 27, ".", "?.".to_owned())])]
    );
    clean(&applied(src, &d));
}

#[test]
fn coalesce_left_not_optional() {
    codes(
        "fn main() { print(1 ?? 2) }",
        &[(Code::CoalesceNotOptional, (1, 19))],
    );
}

#[test]
fn coalesce_never() {
    clean(
        "fn f(x: i64?, c: bool) -> i64 {\n    x ?? if c { return 0 } else { return 1 }\n}\nfn main() {}",
    );
}

#[test]
fn coalesce_type() {
    codes(
        "fn f(x: i64?) -> i64 { x ?? true }\nfn main() {}",
        &[(Code::Mismatch, (1, 29))],
    );
}

#[test]
fn equal_none_on_non_optional() {
    codes(
        "class T {}\nfn f(x: T) -> bool { x == None }\nfn main() {}",
        &[(Code::NeverNone, (2, 22))],
    );
}

#[test]
fn none_equals_none() {
    codes(
        "fn main() { print(None == None) }",
        &[(Code::CannotInfer, (1, 19))],
    );
}

#[test]
fn equal_unit() {
    codes(
        "fn g() {}\nfn main() { print(g() == g()) }",
        &[(Code::NoValue, (2, 19)), (Code::NoValue, (2, 26))],
    );
}

#[test]
fn equal_two_classes() {
    codes(
        "class A {}\nclass B {}\nfn f(a: A, b: B) -> bool { a == b }\nfn main() {}",
        &[(Code::CannotCompare, (3, 28))],
    );
}

#[test]
fn is_none() {
    diagnostic(
        "class T {}\nfn f(x: T?) -> bool { x is None }\nfn main() {}",
        Code::IsTestsNone,
        (2, 23, "x is None"),
        &[],
    );
}

#[test]
fn is_on_integers() {
    diagnostic(
        "fn main() { print(1 is 1) }",
        Code::IsNeedsObjects,
        (1, 19, "1"),
        &[],
    );
}

/// The operand that is not an object is blamed, not the whole expression.
#[test]
fn is_blames_the_operand_that_is_not_an_object() {
    diagnostic(
        "class T {}\nfn f(t: T) -> bool { t is 1 }\nfn main() {}",
        Code::IsNeedsObjects,
        (2, 27, "1"),
        &[],
    );
}

/// A side with no path has no help, as `this value == None` is not code.
#[test]
fn is_none_without_a_path_has_no_help() {
    let d = diagnostic(
        "class T {}\nfn make() -> T? { None }\nfn f() -> bool { make() is None }\nfn main() {}",
        Code::IsTestsNone,
        (3, 18, "make() is None"),
        &[],
    );
    assert_eq!(d.help(), None);
}

/// `t == None` does not compile for a `T`, so it is not offered.
#[test]
fn is_none_on_a_plain_object_has_no_help() {
    let d = diagnostic(
        "class T {}\nfn f(t: T) -> bool { t is None }\nfn main() {}",
        Code::IsTestsNone,
        (2, 22, "t is None"),
        &[],
    );
    assert_eq!(d.help(), None);
}

#[test]
fn is_none_checks_the_other_side() {
    codes(
        "fn main() { print(nope is None) }",
        &[
            (Code::IsTestsNone, (1, 19)),
            (Code::BindingNotFound, (1, 19)),
        ],
    );
}

/// Once an operand is `Error`, the later one is not held to `is`.
#[test]
fn is_after_an_error_checks_no_further() {
    codes(
        "fn main() { print(nope is 1) }",
        &[(Code::BindingNotFound, (1, 19))],
    );
}

#[test]
fn is_two_classes() {
    codes(
        "class A {}\nclass B {}\nfn f(a: A, b: B) -> bool { a is b }\nfn main() {}",
        &[(Code::CannotCompare, (3, 28))],
    );
}

/// The other operand is still checked after a `??` or `?.` fails.
#[test]
fn coalesce_left_not_optional_checks_the_right() {
    codes(
        "fn main() { print(1 ?? nope) }",
        &[
            (Code::CoalesceNotOptional, (1, 19)),
            (Code::BindingNotFound, (1, 24)),
        ],
    );
}

#[test]
fn safe_call_on_non_optional_checks_the_member() {
    codes(
        "class T {\n    v: i64\n}\nfn f(t: T) -> i64? { t?.w }\nfn main() {}",
        &[
            (Code::SafeAccessNotOptional, (4, 22)),
            (Code::NoMember, (4, 25)),
        ],
    );
}

/// The member's argument is checked on the plain receiver too.
#[test]
fn safe_call_on_non_optional_checks_the_arguments() {
    codes(
        "class T {\n    fn g(self, x: i64) -> i64 { x }\n}\nfn f(t: T) -> i64? { t?.g(nope) }\nfn main() {}",
        &[
            (Code::SafeAccessNotOptional, (4, 22)),
            (Code::BindingNotFound, (4, 27)),
        ],
    );
}

/// The class the `?.`, `??`, `==` and `is` lowering tests share.
const C: &str =
    "class T {\n    v: i64\n    o: T?\n    fn m(self) {}\n    fn g(self, x: i64) -> i64 { x }\n}\n";

#[test]
fn lowers_safe_field() {
    f_contains(
        C,
        "fn f(t: T?) -> i64? { t?.v }",
        "(if-some #1 t#0 (block (wrap (. #1 v))) (block None))",
    );
}

#[test]
fn safe_optional_field_not_wrapped() {
    f_contains(
        C,
        "fn f(t: T?) -> T? { t?.o }",
        "(if-some #1 t#0 (block (. #1 o)) (block None))",
    );
}

#[test]
fn lowers_safe_unit_method() {
    f_contains(
        C,
        "fn f(t: T?) { t?.m() }",
        "(if-some #1 t#0 (block (call T.m #1)) (block))",
    );
}

#[test]
fn lowers_safe_method_with_an_argument() {
    f_contains(
        C,
        "fn f(t: T?) -> i64? { t?.g(1) }",
        "(if-some #1 t#0 (block (wrap (call T.g #1 1))) (block None))",
    );
}

#[test]
fn lowers_coalesce() {
    f_contains(
        C,
        "fn f(x: i64?) -> i64 { x ?? 0 }",
        "(if-some #1 x#0 (block #1) (block 0))",
    );
}

#[test]
fn coalesce_with_optional_right() {
    f_contains(
        C,
        "fn f(x: i64?, y: i64?) -> i64? { x ?? y }",
        "(if-some #2 x#0 (block (wrap #2)) (block y#1))",
    );
}

/// A bare `None` on the right takes the left side's type.
#[test]
fn coalesce_with_none_right() {
    f_contains(
        C,
        "fn f(x: i64?) -> i64? { x ?? None }",
        "(if-some #1 x#0 (block (wrap #1)) (block None))",
    );
}

#[test]
fn coalesce_with_none_in_an_if() {
    clean(&format!(
        "{C}fn f(x: i64?, c: bool) -> i64? {{ x ?? if c {{ None }} else {{ 1 }} }}\nfn main() {{}}"
    ));
}

/// `x ?? (y ?? 0)` is an `i64`, which the return type checks.
#[test]
fn coalesce_chain_is_not_optional() {
    clean(&format!(
        "{C}fn f(x: i64?, y: i64?) -> i64 {{ x ?? y ?? 0 }}\nfn main() {{}}"
    ));
}

#[test]
fn equal_wraps_the_plain_side() {
    f_contains(
        C,
        "fn f(a: i64?, b: i64) -> bool { a == b }",
        "(== a#0 (wrap b#1))",
    );
}

/// A bare `None` on the left takes the type of the right side.
#[test]
fn none_on_the_left_takes_the_other_type() {
    f_contains(C, "fn f(t: T?) -> bool { None != t }", "(!= None t#0)");
}

#[test]
fn is_wraps_the_plain_side() {
    f_contains(
        C,
        "fn f(a: T, b: T?) -> bool { a is b }",
        "(is (wrap a#0) b#1)",
    );
}

#[test]
fn spills_is() {
    f_contains(
        C,
        "fn make() -> T { T(v: 0, o: None) }\nfn f(c: bool) -> bool {\n    make() is if c { return false } else { make() }\n}",
        "(block (let #1 (call make)) (let #2 (if c#0 (block (return false)) (block (call make)))) (is #1 #2))",
    );
}

#[test]
fn spills_equal() {
    f_contains(
        C,
        "fn make() -> T { T(v: 0, o: None) }\nfn f(c: bool) -> bool {\n    make() == if c { return false } else { make() }\n}",
        "(block (let #1 (call make)) (let #2 (if c#0 (block (return false)) (block (call make)))) (== #1 #2))",
    );
}

/// `print` of an object names the argument, not the call; a bad argument count stays on the
/// whole call.
#[test]
fn print_argument_errors_sit_on_the_argument() {
    codes(
        "class P {\n    x: i64\n}\nfn nothing() {}\nfn main() {\n    print(P(x: 1))\n    print(nothing())\n    print([1])\n    print(1, 2)\n}",
        &[
            (Code::PrintArgument, (6, 11)),
            (Code::PrintArgument, (7, 5)),
            (Code::PrintArray, (8, 11)),
            (Code::PrintArgument, (9, 5)),
        ],
    );
}

/// Two members of the same kind are "defined twice"; a method and a field of one name name the
/// first one's kind.
#[test]
fn duplicate_member_of_the_same_kind() {
    diagnostic(
        "class A {\n    fn x(self) {}\n    fn x(self) {}\n}\nfn main() {}",
        Code::DefinedTwice,
        (3, 8, "x"),
        &[(2, 8)],
    );
    diagnostic(
        "class A {\n    x: i64\n    x: i64\n}\nfn main() {}",
        Code::DefinedTwice,
        (3, 5, "x"),
        &[(2, 5)],
    );
}

#[test]
fn unit_branch_says_no_value() {
    codes(
        "fn nothing() {}\nfn main() { let y: i64 = if true { 1 } else { nothing() } }",
        &[(Code::NoValue, (2, 47))],
    );
    codes(
        "fn nothing() {}\nfn main() { let y: i64 = if true { nothing() } else { 1 } }",
        &[(Code::NoValue, (2, 36))],
    );
    // Without a type expected, the branch with no value is still the one named.
    codes(
        "fn nothing() {}\nfn main() { let x = if true { nothing() } else { 1 } }",
        &[(Code::NoValue, (2, 31))],
    );
}

/// Branches of two other types still report a mismatch.
#[test]
fn value_branches_of_different_types_still_mismatch() {
    codes(
        "fn main() { let x = if true { 1 } else { false } }",
        &[(Code::Mismatch, (1, 42))],
    );
}

#[test]
fn if_with_two_never_branches_never_produces_a_value() {
    codes(
        "fn f(c: bool) -> i64 {\n    let z = if c { return 1 } else { return 2 }\n    z\n}\nfn main() {}",
        &[(Code::NoValue, (2, 13))],
    );
}

#[test]
fn assigning_to_a_function_names_it() {
    for src in [
        "fn one() {}\nfn main() { one = 2 }",
        "fn one() {}\nfn main() { one += 2 }",
        "fn one() {}\nfn main() { one[0] = 2 }",
    ] {
        diagnostic(src, Code::AssignToFunction, (2, 13, "one"), &[]);
    }
}

/// A local that shadows a function is assigned, not reported.
#[test]
fn local_shadowing_a_function_is_assignable() {
    clean("fn one() {}\nfn main() {\n    let mut one = 1\n    one = 2\n    one += 1\n}");
}

/// The one diagnostic of `src` is `code` at `pos`, with one unsafe fix that replaces its text with
/// `candidate`; applying it parses and leaves no diagnostic of `code`.
fn suggests(src: &str, code: Code, pos: (usize, usize, &str), candidate: &str) {
    let d = diagnostic(src, code, pos, &[]);
    let edit = (pos.0, pos.1, pos.2, candidate.to_owned());
    assert_eq!(fixes(src, &d), [(false, vec![edit])], "{src}");
    let result = applied(src, &d);
    let left: Vec<_> = diags(&result).iter().map(Diagnostic::code).collect();
    assert!(!left.contains(&code), "{result}: {left:?}");
}

/// The one diagnostic of `src` is `code` at `pos`, and has no fix.
fn suggests_nothing(src: &str, code: Code, pos: (usize, usize, &str)) {
    let d = diagnostic(src, code, pos, &[]);
    assert!(d.fixes().is_empty(), "{src}: {d:?}");
}

#[test]
fn missing_function_suggests_the_closest() {
    let src =
        "fn total(xs: [i64]) -> i64 { 0 }\nfn main() {\n    let xs = [1]\n    print(totl(xs))\n}";
    suggests(src, Code::FunctionNotFound, (4, 11, "totl"), "total");
}

#[test]
fn missing_function_suggests_a_class_a_builtin_or_a_conversion() {
    for (name, candidate) in [
        ("prnt", "print"),
        ("pnic", "panic"),
        ("i6", "i64"),
        ("u", "u8"),
    ] {
        let src = format!(
            "class Tree {{\n    value: i64\n}}\nfn main() {{\n    let t = 1\n    {name}(t)\n}}"
        );
        suggests(&src, Code::FunctionNotFound, (6, 5, name), candidate);
    }
}

#[test]
fn class_candidate_is_named_a_class() {
    let src = "class Tree {\n    value: i64\n}\nfn main() {\n    let t = Trea(value: 1)\n    print(t.value)\n}";
    suggests(src, Code::FunctionNotFound, (5, 13, "Trea"), "Tree");
}

#[test]
fn missing_binding_suggests_the_closest_open_binding() {
    suggests(
        "fn main() {\n    let total = 1\n    print(totl)\n}",
        Code::BindingNotFound,
        (3, 11, "totl"),
        "total",
    );
    // A parameter, and a binding of an enclosing block, are open.
    suggests(
        "fn f(count: i64) {\n    let total = 1\n    if true {\n        print(totl)\n    }\n}\nfn main() {}",
        Code::BindingNotFound,
        (4, 15, "totl"),
        "total",
    );
    suggests(
        "fn f(count: i64) {\n    let total = 1\n    if true {\n        print(cont)\n    }\n}\nfn main() {}",
        Code::BindingNotFound,
        (4, 15, "cont"),
        "count",
    );
}

#[test]
fn a_binding_that_left_scope_is_not_a_candidate() {
    suggests_nothing(
        "fn main() {\n    if true {\n        let total = 1\n    }\n    print(totl)\n}",
        Code::BindingNotFound,
        (5, 11, "totl"),
    );
}

#[test]
fn assigning_to_a_missing_binding_suggests_the_closest() {
    suggests(
        "fn main() {\n    let mut total = 1\n    totl = 2\n    print(total)\n}",
        Code::BindingNotFound,
        (3, 5, "totl"),
        "total",
    );
}

#[test]
fn missing_type_suggests_the_closest() {
    for (ty, candidate) in [
        ("Bukcet", "Bucket"),
        ("bol", "bool"),
        ("i6", "i64"),
        ("u", "u8"),
    ] {
        let src = format!("class Bucket {{\n    n: i64\n}}\nfn f(b: {ty}) {{}}\nfn main() {{}}");
        suggests(&src, Code::UnknownType, (4, 9, ty), candidate);
    }
}

#[test]
fn missing_type_inside_an_array_or_optional_suggests_the_closest() {
    suggests(
        "class Bucket {\n    n: i64\n}\nfn f(b: [Bukcet]) {}\nfn main() {}",
        Code::UnknownType,
        (4, 10, "Bukcet"),
        "Bucket",
    );
    suggests(
        "class Bucket {\n    n: i64\n}\nfn f(b: Bukcet?) {}\nfn main() {}",
        Code::UnknownType,
        (4, 9, "Bukcet"),
        "Bucket",
    );
}

#[test]
fn missing_field_suggests_the_closest() {
    suggests(
        "class Point {\n    depth: i64\n    height: i64\n}\nfn main() {\n    let p = Point(depth: 1, height: 2)\n    print(p.dept)\n}",
        Code::NoMember,
        (7, 13, "dept"),
        "depth",
    );
}

#[test]
fn missing_constructor_label_suggests_the_closest_field() {
    suggests(
        "class Point {\n    depth: i64\n}\nfn main() {\n    let p = Point(dept: 1)\n    print(p.depth)\n}",
        Code::NoMember,
        (5, 19, "dept"),
        "depth",
    );
}

#[test]
fn missing_method_suggests_the_closest() {
    suggests(
        "class Point {\n    x: i64\n    fn norm(self) -> i64 { 1 }\n}\nfn main() {\n    let p = Point(x: 1)\n    print(p.nrm())\n}",
        Code::NoMember,
        (7, 13, "nrm"),
        "norm",
    );
}

#[test]
fn missing_array_method_suggests_the_closest() {
    for (name, candidate) in [("lenn", "len"), ("sm", "sum"), ("contain", "contains")] {
        let src = format!("fn main() {{\n    let xs = [1, 2]\n    print(xs.{name}(1))\n}}");
        suggests(&src, Code::NoMember, (3, 14, name), candidate);
    }
}

#[test]
fn a_field_suggests_no_method_and_a_method_no_field() {
    let class = "class Point {\n    depth: i64\n    fn norm(self) -> i64 { 1 }\n}\nfn main() {\n    let p = Point(depth: 1)\n";
    // `dept` is close to the field `depth`, but is called, so only methods are candidates.
    suggests_nothing(
        &format!("{class}    print(p.dept())\n}}"),
        Code::NoMember,
        (7, 13, "dept"),
    );
    suggests_nothing(
        &format!("{class}    print(p.nrm)\n}}"),
        Code::NoMember,
        (7, 13, "nrm"),
    );
}

#[test]
fn no_suggestion_when_nothing_is_close() {
    let class = "class Point {\n    depth: i64\n}\n";
    for (src, code, pos) in [
        (
            "fn total() {}\nfn main() {\n    print(sum(1))\n}".to_owned(),
            Code::FunctionNotFound,
            (3, 11, "sum"),
        ),
        (
            "fn main() {\n    let total = 1\n    print(price)\n}".to_owned(),
            Code::BindingNotFound,
            (3, 11, "price"),
        ),
        (
            format!("{class}fn f(p: Number) {{}}\nfn main() {{}}"),
            Code::UnknownType,
            (4, 9, "Number"),
        ),
        (
            format!("{class}fn main() {{\n    let p = Point(depth: 1)\n    print(p.height)\n}}"),
            Code::NoMember,
            (6, 13, "height"),
        ),
        (
            "fn main() {\n    let xs = [1]\n    print(xs.count())\n}".to_owned(),
            Code::NoMember,
            (3, 14, "count"),
        ),
    ] {
        suggests_nothing(&src, code, pos);
    }
}

/// A member of a type that has none (an `i64`, or a field of an array) has nothing to suggest.
#[test]
fn members_of_a_memberless_type_suggest_nothing() {
    suggests_nothing(
        "fn main() {\n    let n = 1\n    print(n.lenn)\n}",
        Code::NoMember,
        (3, 13, "lenn"),
    );
    suggests_nothing(
        "fn main() {\n    let xs = [1]\n    print(xs.summ)\n}",
        Code::NoMember,
        (3, 14, "summ"),
    );
}

/// Review Focus 5: a function is never offered for a binding, nor a binding for a function.
#[test]
fn suggestions_keep_to_their_kind() {
    suggests_nothing(
        "fn main() {\n    let total = 1\n    totl()\n}",
        Code::FunctionNotFound,
        (3, 5, "totl"),
    );
    suggests_nothing(
        "fn total() {}\nfn main() {\n    print(totl)\n}",
        Code::BindingNotFound,
        (3, 11, "totl"),
    );
}

#[test]
fn with_body_restores_the_outer_body() {
    let src = "";
    let program = parse_ok(src);
    let mut checker = Checker::new(&program, src);
    checker.body.ret = Some(Type::I64);
    let (inner, finished) = checker.with_body(Body::default(), |c| {
        assert_eq!(c.body.ret, None);
        c.body.ret = Some(Type::BOOL);
        c.body.ret
    });
    assert_eq!(inner, Some(Type::BOOL));
    assert_eq!(finished.ret, Some(Type::BOOL));
    assert_eq!(checker.body.ret, Some(Type::I64));
}
