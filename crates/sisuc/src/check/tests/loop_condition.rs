//! W0011: a `while` condition that nothing in the loop changes.

use super::*;

/// `fn f(n: i64)` with `lines` as its body, one to a line, then an empty `main`.
fn f(lines: &[&str]) -> String {
    let body: Vec<_> = lines.iter().map(|line| format!("    {line}")).collect();
    format!("fn f(n: i64) {{\n{}\n}}\nfn main() {{}}", body.join("\n"))
}

/// `program`, then an empty `main`.
fn with_main(program: &str) -> String {
    format!("{program}\nfn main() {{}}")
}

/// Exactly one diagnostic, W0011, at the condition `cond`.
fn warns(src: &str, cond: (usize, usize, &str)) {
    diagnostic(src, Code::UnchangedLoopCondition, cond, &[]);
}

#[test]
fn while_should_warn_when_nothing_changes_its_condition() {
    warns(
        &f(&["let mut i = 0", "while i < n { print(i) }"]),
        (3, 11, "i < n"),
    );
}

#[test]
fn while_should_warn_when_its_local_is_immutable() {
    warns(
        &f(&["let i = 0", "while i < n { print(i) }"]),
        (3, 11, "i < n"),
    );
}

/// A write in the body to something the condition reads stops the warning.
#[test]
fn while_should_not_warn_when_its_body_writes_what_its_condition_reads() {
    for (row, src) in [
        (
            "compound assign to the local",
            f(&["let mut i = 0", "while i < n { i += 1 }"]),
        ),
        (
            "assign to the local",
            f(&["let mut i = 0", "while i < n { i = i + 1 }"]),
        ),
        (
            "write to an element",
            f(&["let mut xs = [n]", "while xs[0] > 0 { xs[0] -= 1 }"]),
        ),
        (
            "append to the local",
            f(&["let mut s = \"\"", "while s.len() < 3 { s += \"a\" }"]),
        ),
    ] {
        clean_row(row, &src);
    }
}

#[test]
fn while_should_not_warn_when_its_condition_reads_a_field() {
    clean(&with_main(
        "class P {\n    x: i64\n}\nfn f(p: P) {\n    while p.x < 3 { print(1) }\n}",
    ));
}

#[test]
fn while_should_not_warn_when_its_condition_calls_a_function() {
    clean(&with_main(
        "fn next(i: i64) -> i64 { i }\nfn f(n: i64) {\n    while next(n) < n { }\n}",
    ));
}

/// `==` on a class compares fields, which `bump` could change through `a`.
#[test]
fn while_should_not_warn_when_it_compares_objects() {
    clean(&with_main(
        "class Node {\n    v: i64\n}\nfn bump(a: Node) { print(a.v) }\nfn f(a: Node, b: Node) {\n    while a != b { bump(a) }\n}",
    ));
}

#[test]
fn while_should_not_warn_when_it_compares_arrays_of_objects() {
    clean(&with_main(
        "class Node {\n    v: i64\n}\nfn bump(a: Node) { print(a.v) }\nfn f(xs: [Node], ys: [Node]) {\n    while xs != ys { bump(xs[0]) }\n}",
    ));
}

/// A generic enum holds what its type arguments hold.
#[test]
fn while_should_not_warn_when_it_compares_generic_enums_of_objects() {
    clean(&with_main(
        "class Node {\n    v: i64\n}\nenum Opt<T> {\n    Has(T)\n    Empty\n}\nfn f(a: Opt<Node>, b: Opt<Node>) {\n    while a != b { print(1) }\n}",
    ));
}

#[test]
fn while_should_not_warn_when_its_enum_holds_a_generic_enum_of_objects() {
    clean(&with_main(
        "class Node {\n    v: i64\n}\nenum Opt<T> {\n    Has(T)\n    Empty\n}\nenum Wrap {\n    W(Opt<Node>)\n}\nfn f(a: Wrap, b: Wrap) {\n    while a != b { print(1) }\n}",
    ));
}

#[test]
fn while_should_warn_when_it_compares_generic_enums_of_numbers() {
    warns(
        &with_main(
            "enum Opt<T> {\n    Has(T)\n    Empty\n}\nfn f(a: Opt<i64>, b: Opt<i64>) {\n    while a != b { print(1) }\n}",
        ),
        (6, 11, "a != b"),
    );
}

#[test]
fn while_should_warn_when_it_compares_an_object_with_none() {
    warns(
        &with_main(
            "class Node {\n    v: i64\n}\nfn f(node: Node?) {\n    while node != None { print(1) }\n}",
        ),
        (5, 11, "node != None"),
    );
}

/// A body that can leave the loop stops the warning.
#[test]
fn while_should_not_warn_when_its_body_can_leave_the_loop() {
    for (row, src) in [
        (
            "break",
            f(&["let i = 0", "while i < n { if n > 5 { break } }"]),
        ),
        (
            "return",
            f(&["let i = 0", "while i < n { if n > 5 { return } }"]),
        ),
        (
            "panic",
            f(&["let i = 0", "while i < n { if n > 5 { panic(\"x\") } }"]),
        ),
    ] {
        clean_row(row, &src);
    }
}

#[test]
fn while_should_not_warn_when_its_body_continues_an_outer_loop() {
    clean(&f(&[
        "outer: for k in 0..n {",
        "    let i = 0",
        "    while i < n { continue outer }",
        "}",
    ]));
}

#[test]
fn while_should_warn_when_its_body_only_calls_a_function() {
    warns(
        &with_main("fn step() { }\nfn f(n: i64) {\n    let i = 0\n    while i < n { step() }\n}"),
        (4, 11, "i < n"),
    );
}

#[test]
fn outer_while_should_warn_when_only_the_inner_loop_changes_its_own_local() {
    warns(
        &with_main(
            "fn f(n: i64, m: i64) {\n    let i = 0\n    while i < n {\n        let mut j = 0\n        while j < m { j += 1 }\n    }\n}",
        ),
        (3, 11, "i < n"),
    );
}

/// The inner loop warns: it writes `i`, which only the outer condition reads.
#[test]
fn while_should_not_warn_when_an_inner_loop_writes_its_local() {
    codes(
        &f(&[
            "let mut i = 0",
            "while i < n {",
            "    while n > 5 { i += 1 }",
            "}",
        ]),
        &[(Code::UnchangedLoopCondition, (4, 15))],
    );
}

#[test]
fn while_should_not_warn_when_its_condition_reads_no_local() {
    clean(&f(&["while 1 < 2 { }"]));
}

#[test]
fn while_should_warn_when_a_local_switch_is_off() {
    warns(
        &with_main("fn f() {\n    let running = false\n    while running { print(1) }\n}"),
        (3, 11, "running"),
    );
}

#[test]
fn while_should_not_warn_when_its_condition_reads_a_constant() {
    clean(&with_main(
        "const RUNNING = false\nfn f() {\n    while RUNNING { print(1) }\n}",
    ));
}

#[test]
fn while_true_should_get_only_its_own_warning() {
    codes(
        &with_main("fn f() {\n    while true { }\n}"),
        &[(Code::WhileTrue, (2, 5))],
    );
}

/// A read of the old `reassigned` flag would see the write before the loop.
#[test]
fn while_should_warn_when_its_local_is_written_only_before_the_loop() {
    warns(
        &f(&["let mut i = 0", "i = 1", "while i < n { print(i) }"]),
        (4, 11, "i < n"),
    );
}

#[test]
fn while_should_warn_when_its_body_shadows_its_local() {
    warns(
        &f(&[
            "let i = 0",
            "while i < n {",
            "    let i = i + 1",
            "    print(i)",
            "}",
        ]),
        (3, 11, "i < n"),
    );
}

#[test]
fn while_should_not_warn_when_its_body_writes_an_immutable_local() {
    codes(
        &f(&["let i = 0", "while i < n { i += 1 }"]),
        &[(Code::AssignToImmutable, (3, 19))],
    );
}

#[test]
fn while_should_not_warn_when_its_body_writes_an_unknown_name() {
    codes(
        &f(&["let i = 0", "while i < n { j += 1 }"]),
        &[(Code::BindingNotFound, (3, 19))],
    );
}

#[test]
fn while_should_not_warn_when_its_body_writes_a_field_of_an_unknown_name() {
    codes(
        &f(&["while n > 0 { q.x = 1 }"]),
        &[(Code::BindingNotFound, (2, 19))],
    );
}

#[test]
fn while_should_not_warn_when_its_condition_calls_a_user_len() {
    clean(&with_main(
        "class Q {\n    n: i64\n    fn len(self) -> i64 { self.n }\n    fn pop(self) { print(1) }\n}\nfn f(q: Q) {\n    while q.len() > 0 { q.pop() }\n}",
    ));
}

#[test]
fn while_should_not_warn_when_its_local_has_an_unknown_type() {
    codes(
        &with_main("fn f(a: Fooo, b: Fooo) {\n    while a != b { print(1) }\n}"),
        &[(Code::UnknownType, (1, 9)), (Code::UnknownType, (1, 18))],
    );
}

#[test]
fn while_should_not_warn_when_it_compares_a_chain_of_strings() {
    clean(&with_main(
        "fn f(a: string, b: string, c: string) {\n    while a < b < c { print(1) }\n}",
    ));
}

#[test]
fn while_should_warn_when_it_compares_a_value_with_an_optional() {
    warns(
        &with_main("fn f(i: i64, limit: i64?) {\n    while i != limit { print(i) }\n}"),
        (2, 11, "i != limit"),
    );
}

#[test]
fn never_reassigned_should_warn_when_the_loop_does_not_name_its_binding() {
    codes(
        &f(&["let mut k = 0", "print(k)", "while n > 0 { print(n) }"]),
        &[
            (Code::NeverReassigned, (2, 13)),
            (Code::UnchangedLoopCondition, (4, 11)),
        ],
    );
}

#[test]
fn while_let_and_for_should_not_warn() {
    clean(&with_main(
        "fn f(o: i64?) {\n    while let x = o { print(x) }\n    for i in 0..3 { print(i) }\n}",
    ));
}

/// E0116 leaves the body checked, so only the error count stops the warning.
#[test]
fn while_should_not_warn_when_its_body_has_an_error() {
    codes(
        &with_main("enum Color { Red }\nfn f(n: i64) {\n    while n > 0 { let Color = 1 }\n}"),
        &[(Code::EnumNamedLocal, (3, 23))],
    );
}

#[test]
fn while_should_not_warn_when_its_body_writes_a_field_of_its_local() {
    clean(&with_main(
        "class Node {\n    mut v: i64\n}\nfn f(p: Node, q: Node) {\n    while p is q { p.v = 1 }\n}",
    ));
}

#[test]
fn while_should_not_warn_when_its_body_writes_a_field_of_an_element() {
    clean(&with_main(
        "class Node {\n    mut v: i64\n}\nfn f(xs: [Node]) {\n    while xs.len() > 0 { xs[0].v = 1 }\n}",
    ));
}

#[test]
fn while_should_not_warn_when_its_body_writes_a_field_of_self() {
    clean(&with_main(
        "class Node {\n    mut v: i64\n    fn f(self, other: Node) {\n        while self is other { self.v = 1 }\n    }\n}",
    ));
}

/// The poison type sits inside the enum, so `item` itself has a type.
#[test]
fn while_should_not_warn_when_its_local_holds_an_unknown_type() {
    codes(
        &with_main(
            "enum Choice {\n    Missing(Fooo)\n    Empty\n}\nfn f(item: Choice?) {\n    while item != None { print(1) }\n}",
        ),
        &[(Code::UnknownType, (2, 13))],
    );
}

#[test]
fn while_should_not_warn_when_its_body_reaches_todo() {
    codes(
        &f(&["let i = 0", "while i < n { if n > 5 { todo } }"]),
        &[(Code::TodoLeft, (3, 30))],
    );
}

#[test]
fn while_should_not_warn_when_its_class_field_has_an_unknown_type() {
    codes(
        &with_main(
            "class Node {\n    value: Fooo\n}\nfn f(node: Node?) {\n    while node != None { print(1) }\n}",
        ),
        &[(Code::UnknownType, (2, 12))],
    );
}

#[test]
fn while_should_not_warn_when_its_enum_holds_a_class_with_an_unknown_type() {
    codes(
        &with_main(
            "class Node {\n    value: Fooo\n}\nenum Choice {\n    SomeNode(Node)\n    Empty\n}\nfn f(item: Choice?) {\n    while item != None { print(1) }\n}",
        ),
        &[(Code::UnknownType, (2, 12))],
    );
}

#[test]
fn while_should_warn_when_its_condition_reads_a_length_and_an_element() {
    warns(
        &f(&["let xs = [n]", "while xs.len() > 0 { print(xs[0]) }"]),
        (3, 11, "xs.len() > 0"),
    );
}

/// The later `let` replaces the first `i` in its scope, so the first one reaches W0002 there.
#[test]
fn never_reassigned_should_skip_a_named_binding_when_a_later_let_shadows_it() {
    codes(
        &f(&[
            "let mut i = 0",
            "while i < n { print(i) }",
            "let i = 5",
            "print(i)",
        ]),
        &[(Code::UnchangedLoopCondition, (3, 11))],
    );
}

#[test]
fn while_should_warn_when_its_condition_concatenates_strings() {
    warns(
        &with_main(
            "fn f(t: string) {\n    let s = \"a\"\n    while s + \"b\" != t { print(s) }\n}",
        ),
        (3, 11, "s + \"b\" != t"),
    );
}

#[test]
fn while_should_warn_when_its_condition_adds_to_an_element() {
    warns(
        &f(&[
            "let xs = [n]",
            "let i = 0",
            "while xs[i] + 1 != n { print(i) }",
        ]),
        (4, 11, "xs[i] + 1 != n"),
    );
}

#[test]
fn while_should_not_warn_when_an_operand_calls_a_function() {
    clean(&with_main(
        "fn g() -> i64 { 1 }\nfn f(n: i64) {\n    let i = 0\n    while i + g() < n { print(i) }\n}",
    ));
}

#[test]
fn while_should_not_warn_when_its_enum_holds_an_enum_that_holds_a_class() {
    clean(&with_main(
        "class Node {\n    v: i64\n}\nenum Inner {\n    Holds(Node)\n}\nenum Outer {\n    Wraps(Inner)\n}\nfn f(a: Outer, b: Outer) {\n    while a != b { print(1) }\n}",
    ));
}

/// The class with the unknown type is not the first one, so its node is not node 0.
#[test]
fn while_should_not_warn_when_its_second_class_has_a_field_of_an_unknown_type() {
    codes(
        &with_main(
            "class Other {\n    v: i64\n}\nclass Node {\n    value: Fooo\n}\nfn f(node: Node?) {\n    while node != None { print(1) }\n}",
        ),
        &[(Code::UnknownType, (5, 12))],
    );
}

/// Every `insert` today comes with a failed expression, which fails the loop body, so no
/// program reaches the count.
#[test]
fn diagnostics_should_count_every_error_it_holds() {
    let mut diagnostics = Diagnostics::default();
    let at = Span::new(0, 0);
    diagnostics.insert(0, DiagnosticKind::IsTestsNone { tested: None, at }.into());
    assert_eq!(diagnostics.errors, 1);
}

/// W0005 reports the comparison, and its fix makes the condition read a method call.
#[test]
fn while_should_not_warn_when_its_condition_compares_with_nan() {
    codes(
        &f(&["let x = 0.5", "while x != f64.nan { print(x) }"]),
        &[(Code::NanComparison, (3, 11))],
    );
}

/// The checker wraps the optional's `nan` in `Wrap`, and W0005 still reports it.
#[test]
fn while_should_not_warn_when_an_optional_compares_with_nan() {
    let rows = [
        ("o != f64.nan", 11),
        ("f64.nan != o", 11),
        ("(o != f64.nan)", 12),
        ("b && o != f64.nan", 16),
    ];
    for (cond, col) in rows {
        let src = with_main(&format!(
            "fn f(o: f64?, b: bool) {{\n    while {cond} {{ print(1) }}\n}}"
        ));
        codes(&src, &[(Code::NanComparison, (2, col))]);
    }
}

/// W0005 names only `f64.nan` itself, so a constant or a negation of it is no W0005.
#[test]
fn while_should_warn_when_it_compares_with_a_nan_that_w0005_does_not_name() {
    warns(
        &with_main("const NAN = f64.nan\nfn f(n: f64) {\n    while n != NAN { print(n) }\n}"),
        (3, 11, "n != NAN"),
    );
    warns(
        &with_main("fn f(n: f64) {\n    while n != -f64.nan { print(n) }\n}"),
        (2, 11, "n != -f64.nan"),
    );
}

#[test]
fn while_should_warn_when_only_its_body_compares_with_nan() {
    codes(
        &f(&[
            "let x = 0.5",
            "let i = 0",
            "while i < n { print(x == f64.nan) }",
        ]),
        &[
            (Code::UnchangedLoopCondition, (4, 11)),
            (Code::NanComparison, (4, 25)),
        ],
    );
}

/// The parser keeps the loop with the format-spec error, and the checker sees only the parser's
/// list of it. The second loop has no error and still warns.
#[test]
fn while_should_not_warn_when_the_parser_reports_an_error_in_it() {
    let src = f(&[
        "while n > 0 {",
        "    print(\"{n:zz}\")",
        "}",
        "while n > 0 {",
        "    print(n)",
        "}",
    ]);
    let ds = crate::front_end::front_end(&src).1;
    let found: Vec<_> = ds.iter().map(|d| (d.code(), at(&src, d.span()))).collect();
    assert_eq!(
        found,
        [
            (Code::UnsupportedFormatSpec, (3, 18)),
            (Code::UnchangedLoopCondition, (5, 11)),
        ]
    );
}
