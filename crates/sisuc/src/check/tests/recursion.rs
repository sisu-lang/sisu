//! W0010: a function or a method that calls itself on every path.

use super::*;

/// `fn f(n: i64) -> i64` with `lines` as its body, one to a line, then an empty `main`.
fn f(lines: &[&str]) -> String {
    let body: Vec<_> = lines.iter().map(|line| format!("    {line}")).collect();
    format!(
        "fn f(n: i64) -> i64 {{\n{}\n}}\nfn main() {{}}",
        body.join("\n")
    )
}

/// `program`, then an empty `main`.
fn with_main(program: &str) -> String {
    format!("{program}\nfn main() {{}}")
}

fn warns(src: &str, name: (usize, usize, &str), calls: &[(usize, usize)]) {
    diagnostic(src, Code::UnconditionalRecursion, name, calls);
}

#[test]
fn function_should_warn_when_every_path_calls_itself() {
    warns(&f(&["f(n)"]), (1, 4, "f"), &[(2, 5)]);
}

#[test]
fn if_should_warn_when_both_branches_call_itself() {
    warns(
        &f(&["if n > 0 { f(n - 1) } else { f(n + 1) }"]),
        (1, 4, "f"),
        &[(2, 16), (2, 34)],
    );
}

#[test]
fn if_should_not_warn_when_one_branch_returns_a_value() {
    clean(&f(&["if n == 0 { 0 } else { f(n - 1) }"]));
}

#[test]
fn return_should_end_a_path_before_the_self_call() {
    clean(&f(&["if n == 0 { return 0 }", "f(n - 1)"]));
}

#[test]
fn panic_should_end_a_path_before_the_self_call() {
    clean(&f(&["if n < 0 { panic(\"x\") }", "f(n - 1)"]));
}

#[test]
fn todo_should_end_a_path_before_the_self_call() {
    codes(
        &f(&["if n < 0 { todo }", "f(n - 1)"]),
        &[(Code::TodoLeft, (2, 16))],
    );
}

#[test]
fn return_should_warn_when_its_operand_calls_itself() {
    warns(&f(&["return f(n - 1)"]), (1, 4, "f"), &[(2, 12)]);
}

#[test]
fn self_call_should_warn_after_a_plain_statement() {
    warns(&f(&["print(n)", "f(n - 1)"]), (1, 4, "f"), &[(3, 5)]);
}

#[test]
fn self_call_should_warn_inside_an_operand() {
    warns(&f(&["1 + f(n - 1)"]), (1, 4, "f"), &[(2, 9)]);
}

#[test]
fn and_should_not_warn_when_only_its_right_side_calls_itself() {
    clean(&with_main("fn f(c: bool) -> bool {\n    c && f(c)\n}"));
}

#[test]
fn or_should_not_warn_when_only_its_right_side_calls_itself() {
    clean(&with_main("fn f(c: bool) -> bool {\n    c || f(c)\n}"));
}

#[test]
fn match_should_warn_when_every_arm_calls_itself() {
    warns(
        &with_main(
            "enum T {\n    Leaf\n    Node(i64)\n}\nfn f(t: T) -> i64 {\n    match t {\n        Leaf => f(t)\n        Node(n) => f(t)\n    }\n}",
        ),
        (5, 4, "f"),
        &[(7, 17), (8, 20)],
    );
}

#[test]
fn match_should_not_warn_when_one_arm_returns_a_value() {
    clean(&with_main(
        "enum T {\n    Leaf\n    Node(i64)\n}\nfn f(t: T) -> i64 {\n    match t {\n        Leaf => 0\n        Node(n) => f(t)\n    }\n}",
    ));
}

#[test]
fn if_let_should_not_warn_when_only_its_body_calls_itself() {
    clean(&with_main(
        "fn f(o: i64?) -> i64 {\n    if let x = o { f(o) } else { 0 }\n}",
    ));
}

#[test]
fn coalesce_should_not_warn_when_only_its_default_calls_itself() {
    clean(&with_main("fn f(o: i64?) -> i64 {\n    o ?? f(o)\n}"));
}

#[test]
fn safe_call_should_not_warn_when_only_the_call_calls_itself() {
    clean(&with_main(
        "class Node {\n    n: i64\n    fn size(self, o: Node?) -> i64 {\n        o?.size(None) ?? 0\n    }\n}",
    ));
}

#[test]
fn loop_should_warn_when_its_body_calls_itself() {
    warns(
        &with_main("fn f() {\n    loop { f() }\n}"),
        (1, 4, "f"),
        &[(2, 12)],
    );
}

#[test]
fn while_should_not_warn_when_its_body_may_not_run() {
    clean(&with_main(
        "fn f(n: i64) {\n    let mut i = n\n    while i > 0 {\n        i -= 1\n        f(i)\n    }\n}",
    ));
}

#[test]
fn while_true_should_warn_when_its_body_calls_itself() {
    codes(
        &with_main("fn f() {\n    while true { f() }\n}"),
        &[
            (Code::UnconditionalRecursion, (1, 4)),
            (Code::WhileTrue, (2, 5)),
        ],
    );
}

#[test]
fn while_should_warn_when_its_condition_calls_itself() {
    warns(
        &with_main("fn f() -> i64 {\n    while f() > 0 { }\n    0\n}"),
        (1, 4, "f"),
        &[(2, 11)],
    );
}

#[test]
fn finishing_loop_should_end_a_path_before_the_self_call() {
    clean(&f(&["let mut i = n", "while i > 0 { i -= 1 }", "f(n)"]));
}

#[test]
fn chain_should_not_warn_when_only_a_later_operand_calls_itself() {
    clean(&f(&["if 0 < n < f(n - 1) { 1 } else { 0 }"]));
}

#[test]
fn function_should_not_warn_when_its_signature_has_an_error() {
    codes(
        &with_main("fn f(a: Fooo) -> i64 {\n    f(1)\n}"),
        &[(Code::UnknownType, (1, 9))],
    );
}

/// `NoMain` is reported at the empty span at offset 0, inside `f`'s declaration.
#[test]
fn function_should_warn_when_it_starts_a_file_without_main() {
    codes(
        "fn f(n: i64) -> i64 { f(n) }",
        &[
            (Code::NoMain, (1, 1)),
            (Code::UnconditionalRecursion, (1, 4)),
        ],
    );
}

#[test]
fn main_should_not_warn_when_its_signature_has_an_error() {
    codes(
        "fn main() -> i64 { main() }",
        &[(Code::MainSignature, (1, 4))],
    );
}

#[test]
fn method_should_warn_when_every_path_calls_itself() {
    warns(
        &with_main("class Node {\n    n: i64\n    fn size(self) -> i64 { self.size() }\n}"),
        (3, 8, "size"),
        &[(3, 28)],
    );
}

#[test]
fn method_should_warn_when_it_calls_itself_on_a_field() {
    warns(
        &with_main(
            "class Node {\n    kids: [Node]\n    fn size(self) -> i64 { self.kids[0].size() }\n}",
        ),
        (3, 8, "size"),
        &[(3, 28)],
    );
}

#[test]
fn mutual_recursion_should_not_warn() {
    clean(&with_main(
        "fn f(n: i64) -> i64 {\n    g(n)\n}\nfn g(n: i64) -> i64 {\n    f(n)\n}",
    ));
}

/// E0116 leaves a typed tree, so only the error search skips `f`.
#[test]
fn function_should_not_warn_when_its_body_has_an_error() {
    codes(
        &with_main("enum Color { Red }\nfn f(n: i64) -> i64 {\n    let Color = 1\n    f(n)\n}"),
        &[(Code::EnumNamedLocal, (3, 9))],
    );
}

#[test]
fn function_should_not_warn_when_its_body_has_no_typed_tree() {
    codes(&f(&["j += 1", "f(n)"]), &[(Code::BindingNotFound, (2, 5))]);
}

#[test]
fn function_should_warn_when_another_function_has_an_error() {
    codes(
        &with_main("fn f(n: i64) -> i64 {\n    f(n)\n}\nfn g() {\n    let i = 0\n    i = 1\n}"),
        &[
            (Code::UnconditionalRecursion, (1, 4)),
            (Code::AssignToImmutable, (6, 5)),
        ],
    );
}

#[test]
fn fib_bench_should_not_warn() {
    clean(include_str!("../../../../../bench/fib/main.sisu"));
}

/// The warning on `f` sorts before `g`'s error, so the search for `g`'s errors must not see it.
#[test]
fn function_should_not_warn_when_its_error_follows_a_warning() {
    codes(
        &with_main(
            "fn f(n: i64) -> i64 { f(n) }\nenum Color { Red }\nfn g(n: i64) -> i64 {\n    let Color = 1\n    g(n)\n}",
        ),
        &[
            (Code::UnconditionalRecursion, (1, 4)),
            (Code::EnumNamedLocal, (4, 9)),
        ],
    );
}

/// `Leaf.size` and `Node.size` share a name but not a `FuncId`.
#[test]
fn method_should_not_warn_when_it_calls_the_same_name_on_another_class() {
    clean(&with_main(
        "class Leaf {\n    n: i64\n    fn size(self) -> i64 { self.n }\n}\nclass Node {\n    child: Leaf\n    fn size(self) -> i64 { self.child.size() }\n}",
    ));
}

#[test]
fn nested_self_call_should_mark_only_the_inner_call() {
    warns(&f(&["f(f(n))"]), (1, 4, "f"), &[(2, 7)]);
}

#[test]
fn and_should_warn_when_its_left_side_calls_itself() {
    warns(
        &with_main("fn f(c: bool) -> bool {\n    f(c) && c\n}"),
        (1, 4, "f"),
        &[(2, 5)],
    );
}

/// The inner call is found first, but it starts after the outer one.
#[test]
fn secondaries_should_be_in_source_order_when_an_argument_calls_itself() {
    warns(
        &f(&["f(if n > 0 { f(0) } else { 0 })"]),
        (1, 4, "f"),
        &[(2, 5), (2, 18)],
    );
}

/// The index runs before the value, so `f(2)` never runs.
#[test]
fn element_write_should_mark_the_call_that_runs_first() {
    warns(
        &f(&["let mut xs = [0]", "xs[f(1)] = f(2)", "0"]),
        (1, 4, "f"),
        &[(3, 8)],
    );
}
