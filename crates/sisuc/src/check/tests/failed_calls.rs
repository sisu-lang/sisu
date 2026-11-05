//! A failed call whose callee fixes its result type is typed poison: the code that uses the
//! value reports a mismatch with that type. A callee whose result an argument decides stays
//! `Error`, as does a call that fails its block bound.

use super::*;
use Code::{ArgumentCount, ArgumentNeedsLabel, Mismatch, NotConform};

/// The declarations `decls`, then a function `f` whose body is the statement `body`, then `main`.
fn program(decls: &str, body: &str) -> String {
    format!("{decls}\nfn f() {{\n    {body}\n}}\nfn main() {{}}")
}

/// The line of the statement of `program(decls, _)`: the line after `fn f() {`.
fn line_of(decls: &str) -> usize {
    decls.lines().count() + 2
}

/// Asserts the diagnostics of `body` after `decls`: a `0` line in `expected` stands for the
/// line of the statement.
fn failed(decls: &str, body: &str, expected: &[Wanted]) {
    let line = line_of(decls);
    let want: Vec<Wanted> = expected
        .iter()
        .map(|&(code, (l, col))| (code, (if l == 0 { line } else { l }, col)))
        .collect();
    codes(&program(decls, body), &want);
}

/// The column of the value in `let n: i64 = `, after the four spaces of the indent.
const VALUE: usize = 18;

#[test]
fn a_function_with_the_wrong_count_should_keep_its_result_type() {
    failed(
        "fn g(n: i64) -> [string] { [] }",
        "let n: i64 = g(1, 2)",
        &[(ArgumentCount, (0, VALUE)), (Mismatch, (0, VALUE))],
    );
}

#[test]
fn a_function_with_a_wrong_argument_should_keep_its_result_type() {
    failed(
        "fn g(n: i64) -> [string] { [] }",
        "let n: i64 = g(\"s\")",
        &[(Mismatch, (0, VALUE)), (Mismatch, (0, VALUE + 2))],
    );
}

#[test]
fn a_method_with_the_wrong_count_should_keep_its_result_type() {
    failed(
        "class Node {\n    v: i64\n    fn ys(self, n: i64) -> [string] { [] }\n}",
        "let n: i64 = Node(v: 1).ys(1, 2)",
        &[(ArgumentCount, (0, VALUE)), (Mismatch, (0, VALUE))],
    );
}

#[test]
fn a_method_of_a_generic_class_should_keep_the_result_of_its_receiver_arguments() {
    failed(
        "class Bx<T> {\n    v: T\n    fn get(self, n: i64) -> T { self.v }\n}",
        "let s: string = Bx(v: 1).get(1, 2)",
        &[(ArgumentCount, (0, VALUE + 3)), (Mismatch, (0, VALUE + 3))],
    );
    failed(
        "class Bx<T> {\n    v: T\n    fn get(self, n: i64) -> T { self.v }\n}",
        "let s: string = Bx(v: 1).get(\"s\")",
        &[(Mismatch, (0, VALUE + 3)), (Mismatch, (0, VALUE + 3 + 13))],
    );
}

#[test]
fn an_interface_requirement_with_the_wrong_count_should_keep_its_result_type() {
    let decls = "interface Named {\n    fn name(self) -> string\n}\nclass Cat: Named {\n    fn name(self) -> string { \"cat\" }\n}\nfn show(n: any Named) {\n    let k: i64 = n.name(1)\n}";
    codes(
        &format!("{decls}\nfn main() {{}}"),
        &[(ArgumentCount, (8, 18)), (Mismatch, (8, 18))],
    );
}

#[test]
fn a_function_with_written_type_arguments_should_keep_the_result_they_fix() {
    let decls = "fn gen<T>(t: T) -> [T] { [t] }";
    failed(
        decls,
        "let s: string = gen<i64>(1, 2)",
        &[(ArgumentCount, (0, VALUE + 3)), (Mismatch, (0, VALUE + 3))],
    );
    failed(
        decls,
        "let s: string = gen<i64>(\"s\")",
        &[(Mismatch, (0, VALUE + 3)), (Mismatch, (0, VALUE + 3 + 9))],
    );
}

#[test]
fn a_constructor_with_a_mistake_should_keep_its_class() {
    let decls = "class Pair {\n    a: [string]\n}";
    failed(
        decls,
        "let n: i64 = Pair([], [])",
        &[(Mismatch, (0, VALUE)), (ArgumentNeedsLabel, (0, VALUE + 5))],
    );
    failed(
        decls,
        "let n: i64 = Pair(a: 1)",
        &[(Mismatch, (0, VALUE)), (Mismatch, (0, VALUE + 8))],
    );
}

#[test]
fn a_variant_with_a_mistake_should_keep_its_enum() {
    let decls = "enum Opt {\n    Wrap([string])\n    Nope\n}";
    failed(
        decls,
        "let n: i64 = Opt.Wrap([], [])",
        &[(ArgumentCount, (0, VALUE)), (Mismatch, (0, VALUE))],
    );
    failed(
        decls,
        "let n: i64 = Opt.Wrap(1)",
        &[(Mismatch, (0, VALUE)), (Mismatch, (0, VALUE + 9))],
    );
}

#[test]
fn a_built_in_method_with_the_wrong_count_should_keep_its_result_type() {
    for (row, body) in [
        ("array", "let xs = [1]\n    let s: string = xs.len(1)"),
        ("string", "let t = \"a\"\n    let s: string = t.len(1)"),
        ("number", "let x = 1.5\n    let s: string = x.sqrt(1)"),
    ] {
        let src = program("fn pad() {}", body);
        let found: Vec<_> = diags(&src).iter().map(Diagnostic::code).collect();
        assert_eq!(found, [ArgumentCount, Mismatch], "row `{row}`");
    }
}

#[test]
fn a_prelude_function_with_the_wrong_count_should_keep_its_result_type() {
    for (row, body) in [
        ("read_line", "let n: i64 = read_line(1)"),
        ("panic", "let s: string = panic(\"a\", \"b\")"),
        ("print", "let s: string = print(1, 2)"),
    ] {
        let src = program("fn pad() {}", body);
        let found: Vec<_> = diags(&src).iter().map(Diagnostic::code).collect();
        let want = match row {
            "panic" => vec![ArgumentCount, Code::NoValue],
            "print" => vec![Code::PrintArgument, Code::NoValue],
            _ => vec![ArgumentCount, Mismatch],
        };
        assert_eq!(found, want, "row `{row}`");
    }
}

#[test]
fn a_generic_function_with_an_unbound_result_should_stay_error_when_it_fails() {
    failed(
        "fn gen<T>(t: T) -> [T] { [t] }",
        "let s: string = gen(1, 2)",
        &[(ArgumentCount, (0, VALUE + 3))],
    );
}

#[test]
fn a_call_that_fails_its_block_bound_should_stay_error_when_the_count_is_wrong() {
    with_prelude();
    let src = "class Node {}\nfn f(xs: [Node], a: Node) {\n    let s: string = xs.contains(a, a)\n}\nfn main() {}";
    codes(src, &[(ArgumentCount, (3, 21))]);
    let fixed = "class Node {}\nfn f(xs: [Node], a: Node) {\n    let s: string = xs.contains(a)\n}\nfn main() {}";
    codes(fixed, &[(NotConform, (3, 21))]);
}

#[test]
fn an_interface_requirement_with_written_type_arguments_should_keep_the_result_they_fix() {
    let decls = "interface Keep {\n    fn keep<U>(self, x: U) -> [U]\n}\nfn show<T: Keep>(t: T) {\n    let s: string = t.keep<i64>(true)\n}";
    codes(
        &format!("{decls}\nfn main() {{}}"),
        &[(Mismatch, (5, 21)), (Mismatch, (5, 33))],
    );
}

#[test]
fn a_function_with_a_broken_result_type_should_stay_error_when_it_fails() {
    let decls = "fn g(n: i64) -> Nope { panic(\"no\") }";
    let src = program(decls, "let s: string = g(1, 2)");
    let found: Vec<_> = diags(&src).iter().map(Diagnostic::code).collect();
    assert_eq!(found, [Code::UnknownType, ArgumentCount]);
}
