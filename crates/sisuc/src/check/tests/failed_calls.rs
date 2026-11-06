//! A failed call whose callee fixes its result type is typed poison: the code that uses the
//! value reports a mismatch with that type. A callee whose result an argument decides stays
//! `Error`, as does a call that fails its block bound.

use super::*;
use Code::{
    ArgumentCount, ArgumentNeedsLabel, AssignToExpression, BindingNotFound, LabeledArgument,
    MayBeNone, Mismatch, MissingField, NoValue, NotConform, PrintArgument, TypeArgument,
    TypeArguments, UnknownType,
};

/// The declarations `decls`, then a function `f` whose body is the statement `body`, then `main`.
fn program(decls: &str, body: &str) -> String {
    program_in(decls, "f()", body)
}

/// `program`, where the function is `fn header { body }`.
fn program_in(decls: &str, header: &str, body: &str) -> String {
    format!("{decls}\nfn {header} {{\n    {body}\n}}\nfn main() {{}}")
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
    for (row, body, wanted) in [
        (
            "read_line",
            "let n: i64 = read_line(1)",
            [ArgumentCount, Mismatch],
        ),
        (
            "panic",
            "let s: string = panic(\"a\", \"b\")",
            [ArgumentCount, NoValue],
        ),
        (
            "print",
            "let s: string = print(1, 2)",
            [PrintArgument, NoValue],
        ),
    ] {
        let src = program("fn pad() {}", body);
        let found: Vec<_> = diags(&src).iter().map(Diagnostic::code).collect();
        assert_eq!(found, wanted, "row `{row}`");
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
    assert_eq!(found, [UnknownType, ArgumentCount]);
}

/// The column of the call in `let s: string = `, after the four spaces of the indent.
const CALL: usize = 21;

/// A row of a table: its name, its statement, and each diagnostic wanted, as its code and its
/// column.
type Row<'a> = (&'a str, &'a str, &'a [(Code, usize)]);

/// Asserts the diagnostics of each row of `table`: the statement of the row in the function
/// `fn header { ... }`, after `decls`.
fn rows(decls: &str, header: &str, table: &[Row<'_>]) {
    let line = line_of(decls);
    for &(row, body, wanted) in table {
        let want: Vec<Wanted> = wanted.iter().map(|&(c, col)| (c, (line, col))).collect();
        codes_row(row, &program_in(decls, header, body), &want);
    }
}

#[test]
fn a_count_error_should_not_type_a_call_whose_bound_fails() {
    let decls = "class Node {}\nclass N {}\nfn bq<T: Eq>(t: T) -> [T] { [t] }\nfn hb<T: Eq>(t: T) -> [string] { [] }\nfn ge<T: Eq>(n: T) -> T { n }\nfn h<T: Eq>(n: T) -> [string] { [] }\nclass Bx<T> {\n    v: T\n    fn mk<U: Eq>(self, u: U) -> T { self.v }\n}\ninterface Keep {\n    fn keep<U: Eq>(self, x: U) -> [U]\n}";
    rows(
        decls,
        "f<T: Keep>(t: T)",
        &[
            (
                "a1",
                "let s: string = bq<Node>(Node(), 2)",
                &[(ArgumentCount, CALL)],
            ),
            (
                "a2",
                "let s: string = hb(Node(), 1)",
                &[(ArgumentCount, CALL)],
            ),
            (
                "a3",
                "let s: string = Bx(v: 1).mk<Node>(Node(), 2)",
                &[(ArgumentCount, CALL)],
            ),
            (
                "a4",
                "let s: string = t.keep<Node>(Node(), 1)",
                &[(ArgumentCount, CALL)],
            ),
            ("a5", "let s: string = ge<N>()", &[(ArgumentCount, CALL)]),
            // The corrected call gives E0032 as well: the arguments decide its own parameter.
            (
                "a6",
                "let s: string = Bx(v: 1).mk(1, 2)",
                &[(ArgumentCount, CALL)],
            ),
            ("a9", "let s: string = hb(1, 2)", &[(ArgumentCount, CALL)]),
            // Guards: the bound holds, so the call keeps its type.
            (
                "a7",
                "let s: string = ge<i64>(1, 2)",
                &[(ArgumentCount, CALL), (Mismatch, CALL)],
            ),
            (
                "a8",
                "let s: string = h<i64>(1, 2)",
                &[(ArgumentCount, CALL), (Mismatch, CALL)],
            ),
        ],
    );
}

#[test]
fn a_count_error_should_not_type_a_call_whose_written_list_fails() {
    let decls = "fn h2<T>(t: T) -> [string] { [] }\nfn gen<T>(t: T) -> [T] { [t] }\nfn k() -> [string] { [] }\nclass B<T> {\n    v: T\n    fn g<U>(self, n: U) -> T { self.v }\n}";
    rows(
        decls,
        "f()",
        &[
            (
                "b1",
                "let s: string = h2<i64, i64>(1, 2)",
                &[(ArgumentCount, CALL), (TypeArguments, CALL + 2)],
            ),
            (
                "b2",
                "let s: string = B(v: 1).g<Nope>()",
                &[(ArgumentCount, CALL), (UnknownType, CALL + 10)],
            ),
            (
                "b3",
                "let s: string = B(v: 1).g<i64, bool>()",
                &[(ArgumentCount, CALL), (TypeArguments, CALL + 9)],
            ),
            (
                "b4",
                "let s: string = B(v: 1).g<unit>()",
                &[(ArgumentCount, CALL), (TypeArgument, CALL + 10)],
            ),
            (
                "b5",
                "let s: string = h2<Nope>(1, 2)",
                &[(ArgumentCount, CALL), (UnknownType, CALL + 3)],
            ),
            // Guards: `gen` reports its list already, and `k` takes none.
            (
                "b6",
                "let s: string = gen<i64, i64>(1, 2)",
                &[(ArgumentCount, CALL), (TypeArguments, CALL + 3)],
            ),
            (
                "b7",
                "let s: string = k<i64>(1)",
                &[
                    (ArgumentCount, CALL),
                    (Mismatch, CALL),
                    (TypeArguments, CALL + 1),
                ],
            ),
        ],
    );
}

#[test]
fn a_built_in_call_with_a_label_or_argument_error_should_keep_its_result_type() {
    let decls = "enum E: Eq {\n    A\n    B\n}\nfn u() {}";
    rows(
        decls,
        "f(b: f64, e: E)",
        &[
            (
                "c1",
                "let s: string = b.pow(n: 1.0)",
                &[(Mismatch, CALL), (LabeledArgument, CALL + 6)],
            ),
            (
                "c3",
                "let s: string = e.eq(1)",
                &[(Mismatch, CALL), (Mismatch, CALL + 5)],
            ),
            (
                "c4",
                "let s: string = print(n: 1)",
                &[(NoValue, CALL), (LabeledArgument, CALL + 6)],
            ),
            (
                "c5",
                "let s: string = panic(n: \"x\")",
                &[(NoValue, CALL), (LabeledArgument, CALL + 6)],
            ),
            (
                "c7",
                "let s: string = print(u())",
                &[(NoValue, CALL), (PrintArgument, CALL + 6)],
            ),
            (
                "c8",
                "let s: string = panic(1)",
                &[(NoValue, CALL), (Mismatch, CALL + 6)],
            ),
        ],
    );
}

#[test]
fn a_generic_construction_with_a_written_list_should_keep_its_type_when_it_fails() {
    let decls = "class B<T> {\n    v: T\n}\nenum E<T> {\n    V(T)\n}\ntype C = E<i64>";
    rows(
        decls,
        "f()",
        &[
            (
                "d1",
                "let s: string = B<i64>()",
                &[(MissingField, CALL), (Mismatch, CALL)],
            ),
            (
                "d3",
                "let s: string = B<i64>(v: \"x\")",
                &[(Mismatch, CALL), (Mismatch, CALL + 10)],
            ),
            (
                "d4",
                "let s: string = E<i64>.V()",
                &[(ArgumentCount, CALL), (Mismatch, CALL)],
            ),
            (
                "d7",
                "let s: string = Some<i64>(1, 2)",
                &[(ArgumentCount, CALL), (Mismatch, CALL)],
            ),
            (
                "d10",
                "let s: string = C.V()",
                &[(ArgumentCount, CALL), (Mismatch, CALL)],
            ),
        ],
    );
}

#[test]
fn a_failed_construction_should_not_type_a_call_whose_bound_or_written_list_fails() {
    let decls = "class Node {}\nclass B<T> {\n    v: T\n}\nclass Bq<T: Eq> {\n    v: T\n}\nenum Eq2<T: Eq> {\n    V(T)\n}\nclass Bq2<T: Eq> {\n    a: T\n    b: i64\n}\ntype A<T> = Bq2<T>";
    rows(
        decls,
        "f()",
        &[
            // Guards: the bound fails, so the corrected call is `Error` as well.
            ("d13", "let s: string = Bq<Node>()", &[(MissingField, CALL)]),
            (
                "d14",
                "let s: string = Eq2<Node>.V()",
                &[(ArgumentCount, CALL)],
            ),
            (
                "d15",
                "let s: string = B<Nope>()",
                &[(MissingField, CALL), (UnknownType, CALL + 2)],
            ),
            // The bound that the alias's body puts on `T` fails, and an argument fails too.
            (
                "d16",
                "let s: string = A(a: Node(), b: \"x\")",
                &[(NotConform, CALL), (Mismatch, CALL + 16)],
            ),
        ],
    );
}

#[test]
fn a_class_construction_through_an_alias_should_not_keep_its_type_when_a_bound_fails() {
    let decls = "class Node {}\nclass Bq2<T: Eq> {\n    a: T\n    b: i64\n}\ntype A<T> = Bq2<T>";
    rows(
        decls,
        "f()",
        &[
            // The bound that the alias's body puts on `T` fails, and an argument has no value.
            (
                "d17",
                "let s: string = A(a: Node(), b: missing)",
                &[(NotConform, CALL), (BindingNotFound, CALL + 16)],
            ),
        ],
    );
}

#[test]
fn a_some_through_an_alias_should_keep_the_alias_type_when_it_fails() {
    let decls = "type G<T> = T\ntype W<T> = [T]?\ntype O = i64?\ntype P<T> = T?\ntype MM<T> = T??";
    rows(
        decls,
        "f()",
        &[
            (
                "s1",
                "let s: string = W<i64>.Some(1, 2)",
                &[(ArgumentCount, CALL), (Mismatch, CALL)],
            ),
            (
                "s2",
                "let s: string = O.Some(1, 2)",
                &[(ArgumentCount, CALL), (Mismatch, CALL)],
            ),
            (
                "s3",
                "let s: string = G<i64?>.Some(1, 2)",
                &[(ArgumentCount, CALL), (Mismatch, CALL)],
            ),
            (
                "s4",
                "let s: string = W<i64>.Some(n: [1])",
                &[(Mismatch, CALL), (LabeledArgument, CALL + 12)],
            ),
            (
                "s5",
                "let s: string = O.Some(n: 1)",
                &[(Mismatch, CALL), (LabeledArgument, CALL + 7)],
            ),
            (
                "s6",
                "let s: string = W<i64>.Some(\"a\")",
                &[(Mismatch, CALL), (Mismatch, CALL + 12)],
            ),
            (
                "s7",
                "let s: string = O.Some(\"a\")",
                &[(Mismatch, CALL), (Mismatch, CALL + 7)],
            ),
            // An E0062 comes first at its start (see `order`).
            (
                "s8",
                "W<i64>.Some(1, 2).push(\"a\")",
                &[(MayBeNone, 5), (ArgumentCount, 5), (AssignToExpression, 5)],
            ),
            (
                "s9",
                "let s: string = P<i64>.Some(1, 2)",
                &[(ArgumentCount, CALL), (Mismatch, CALL)],
            ),
            (
                "s10",
                "let s: string = MM<i64>.Some(n: Some(1))",
                &[(Mismatch, CALL), (LabeledArgument, CALL + 13)],
            ),
        ],
    );
}
