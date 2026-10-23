//! W0012: a function, method, class, enum or constant that no live code uses.

use super::*;

const W0012: Code = Code::UnusedItem;

/// `old_report` is the only caller of `format_row`, and nothing calls `old_report`.
const CHAIN: &str = "fn format_row() {\n    print(1)\n}\nfn old_report() {\n    format_row()\n}\n";

/// A class `P` with a field `x` and a method `reset`.
const CLASS_P: &str = "class P {\n    x: i64\n    fn reset(self) {\n        print(1)\n    }\n}\n";

#[test]
fn function_should_warn_when_nothing_calls_it() {
    judge_unused();
    diagnostic(
        "fn helper() {\n    print(1)\n}\nfn main() {}",
        W0012,
        (1, 4, "helper"),
        &[],
    );
}

#[test]
fn function_should_warn_when_only_a_dead_function_calls_it() {
    judge_unused();
    assert_eq!(
        found(&format!("{CHAIN}fn main() {{}}")),
        [(W0012, (1, 4), vec![(5, 5)]), (W0012, (4, 4), vec![])]
    );
}

#[test]
fn function_should_not_warn_when_a_live_function_also_calls_it() {
    judge_unused();
    assert_eq!(
        found(&format!("{CHAIN}fn main() {{\n    format_row()\n}}")),
        [(W0012, (4, 4), vec![])]
    );
}

#[test]
fn function_should_warn_when_only_it_calls_itself() {
    judge_unused();
    diagnostic(
        "fn f(n: i64) -> i64 {\n    if n == 0 { 0 } else { f(n - 1) }\n}\nfn main() {}",
        W0012,
        (1, 4, "f"),
        &[],
    );
}

#[test]
fn functions_should_warn_when_they_only_call_each_other() {
    judge_unused();
    assert_eq!(
        found("fn a() {\n    b()\n}\nfn b() {\n    a()\n}\nfn main() {}"),
        [(W0012, (1, 4), vec![(5, 5)]), (W0012, (4, 4), vec![(2, 5)])]
    );
}

#[test]
fn method_should_warn_when_nothing_calls_it() {
    judge_unused();
    diagnostic(
        &format!("{CLASS_P}fn main() {{\n    let p = P(x: 1)\n    print(p.x)\n}}"),
        W0012,
        (3, 8, "reset"),
        &[],
    );
}

#[test]
fn class_should_warn_when_only_a_type_names_it() {
    judge_unused();
    // One warning: the unbuilt class hides its dead method.
    diagnostic(
        &format!(
            "{CLASS_P}fn show(p: P?) {{\n    print(p == None)\n}}\nfn main() {{\n    show(None)\n}}"
        ),
        W0012,
        (1, 7, "P"),
        &[],
    );
}

#[test]
fn enum_should_cite_a_dead_build_of_its_variant() {
    judge_unused();
    let src = "enum Light {\n    Red\n    Green\n}\nfn old_report() {\n    let _ = Light.Red\n}\nfn main() {}";
    assert_eq!(
        found(src),
        [(W0012, (1, 6), vec![(6, 13)]), (W0012, (5, 4), vec![])]
    );
}

#[test]
fn enum_should_warn_when_nothing_builds_a_variant() {
    judge_unused();
    // `pick` calls `name`, so only the enum warns.
    diagnostic(
        "enum E {\n    A\n    B\n    fn name(self) -> i64 {\n        1\n    }\n}\nfn pick(e: E?) -> i64 {\n    e?.name() ?? 0\n}\nfn main() {\n    print(pick(None))\n}",
        W0012,
        (1, 6, "E"),
        &[],
    );
}

#[test]
fn items_should_cite_their_own_dead_uses_when_they_are_not_the_first_of_their_kind() {
    judge_unused();
    // `B` is the second class, `Light` comes after the classes and `LIMIT` is the second constant.
    let src = "class A {\n    x: i64\n}\nclass B {\n    y: i64\n}\nenum Light {\n    Red\n}\nconst ONE = 1\nconst LIMIT = 10\nfn old_report() {\n    print(B(y: LIMIT).y)\n    print(Light.Red == Light.Red)\n}\nfn main() {\n    print(ONE)\n}";
    assert_eq!(
        found(src),
        [
            (W0012, (1, 7), vec![]),
            (W0012, (4, 7), vec![(13, 11)]),
            (W0012, (7, 6), vec![(14, 11), (14, 24)]),
            (W0012, (11, 7), vec![(13, 16)]),
            (W0012, (12, 4), vec![]),
        ]
    );
}

#[test]
fn chained_calls_should_be_cited_in_check_order_when_they_start_at_one_place() {
    judge_unused();
    // Each call of the chain starts at `node`, and the innermost call is checked first.
    let src = "class Node {\n    n: i64\n    fn next(self) -> Node {\n        self\n    }\n}\nfn old_report(node: Node) {\n    print(node.next().next().next().next().n)\n}\nfn main() {\n    print(Node(n: 1).n)\n}";
    let (_, ds) = checked(src);
    let cited: Vec<Vec<&str>> = (ds.iter())
        .map(|d| d.secondary().iter().map(|(s, _)| &src[s.range()]).collect())
        .collect();
    assert_eq!(
        cited,
        [
            vec![
                "node.next()",
                "node.next().next()",
                "node.next().next().next()"
            ],
            vec![],
        ]
    );
}

#[test]
fn constant_should_warn_when_nothing_names_it() {
    judge_unused();
    diagnostic(
        "const LIMIT = 10\nfn main() {}",
        W0012,
        (1, 7, "LIMIT"),
        &[],
    );
}

#[test]
fn constant_should_not_warn_when_a_live_constant_names_it() {
    judge_unused();
    clean("const A = 1\nconst B = A + 1\nfn main() {\n    print(B)\n}");
}

#[test]
fn constant_should_warn_when_only_a_dead_function_names_it() {
    judge_unused();
    assert_eq!(
        found("const LIMIT = 10\nfn old_report() {\n    print(LIMIT)\n}\nfn main() {}"),
        [(W0012, (1, 7), vec![(3, 11)]), (W0012, (2, 4), vec![])]
    );
}

#[test]
fn constant_should_cite_its_first_three_dead_uses_in_source_order() {
    judge_unused();
    // `OTHER` is checked before the functions, but its use comes last in the source.
    let src = "const LIMIT = 10\nfn a() {\n    print(LIMIT)\n}\nfn b() {\n    print(LIMIT)\n}\nfn c() {\n    print(LIMIT)\n}\nconst OTHER = LIMIT + 1\nfn main() {}";
    assert_eq!(
        found(src),
        [
            (W0012, (1, 7), vec![(3, 11), (6, 11), (9, 11)]),
            (W0012, (2, 4), vec![]),
            (W0012, (5, 4), vec![]),
            (W0012, (8, 4), vec![]),
            (W0012, (11, 7), vec![]),
        ]
    );
}

#[test]
fn underscore_function_should_not_warn() {
    judge_unused();
    clean("fn _helper() {\n    print(1)\n}\nfn main() {}");
}

#[test]
fn underscore_class_should_not_warn_when_nothing_builds_it() {
    judge_unused();
    // A class is never a root, so only the report skips its `_` name.
    clean("class _Pad {\n    x: i64\n}\nfn main() {}");
}

#[test]
fn underscore_function_should_keep_what_it_calls_live() {
    judge_unused();
    clean(&format!(
        "{}fn main() {{}}",
        CHAIN.replace("old_report", "_old_report")
    ));
}

#[test]
fn dead_function_should_not_warn_when_the_program_has_an_error() {
    judge_unused();
    codes(
        "fn helper() {\n    print(1)\n}\nfn main() {\n    let t = 1\n    let u: i64 = \"a\"\n}",
        &[(Code::Mismatch, (6, 18))],
    );
}

#[test]
fn dead_function_should_not_warn_when_its_call_fails() {
    judge_unused();
    // Without the gate, `other` warns: its own check does not fail.
    codes(
        "fn helper() {\n    print(1)\n}\nfn main() {\n    let t = 1\n    helper(1)\n}\nfn other() {\n    print(2)\n}",
        &[(Code::ArgumentCount, (6, 5))],
    );
}

#[test]
fn generic_function_should_not_warn_when_main_calls_it() {
    judge_unused();
    clean("fn first<T>(xs: [T]) -> T {\n    xs[0]\n}\nfn main() {\n    print(first([1, 2]))\n}");
}

#[test]
fn safe_call_should_keep_its_method_live() {
    judge_unused();
    clean(
        "class Node {\n    n: i64\n    fn size(self) -> i64 {\n        self.n\n    }\n    fn total(self) -> i64 {\n        self.size()\n    }\n}\nfn count(o: Node?) -> i64 {\n    o?.total() ?? 0\n}\nfn main() {\n    print(count(Node(n: 1)))\n}",
    );
}
