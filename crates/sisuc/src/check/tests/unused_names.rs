//! W0012: a function, method, class, enum or constant that no live code uses. W0013: a field
//! that no live code reads, or a variant that no live code builds.

use super::*;

const W0012: Code = Code::UnusedItem;
const W0013: Code = Code::UnusedMember;

/// `old_report` is the only caller of `format_row`, and nothing calls `old_report`.
const CHAIN: &str = "fn format_row() {\n    print(1)\n}\nfn old_report() {\n    format_row()\n}\n";

/// A class `P` with the fields `x` and `y`.
const P: &str = "class P {\n    x: i64\n    y: i64\n}\n";

/// A class `P` with the field `x`.
const P_X: &str = "class P {\n    x: i64\n}\n";

/// An enum `Light` with the variants `Red` and `Green`.
const LIGHT: &str = "enum Light {\n    Red\n    Green\n}\n";

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

#[test]
fn field_should_warn_when_nothing_reads_it() {
    judge_unused();
    diagnostic(
        &format!("{P}fn main() {{\n    let p = P(x: 1, y: 2)\n    print(p.x)\n}}"),
        W0013,
        (3, 5, "y"),
        &[],
    );
}

#[test]
fn field_should_warn_when_code_only_writes_it() {
    judge_unused();
    diagnostic(
        "class P {\n    x: i64\n    mut y: i64\n}\nfn main() {\n    let p = P(x: 1, y: 2)\n    p.y = 3\n    print(p.x)\n}",
        W0013,
        (3, 9, "y"),
        &[],
    );
}

#[test]
fn field_should_warn_when_code_only_compound_assigns_it() {
    judge_unused();
    diagnostic(
        "class P {\n    x: i64\n    mut n: i64\n}\nfn main() {\n    let p = P(x: 1, n: 0)\n    p.n += 1\n    print(p.x)\n}",
        W0013,
        (3, 9, "n"),
        &[],
    );
}

#[test]
fn equal_should_read_every_field() {
    judge_unused();
    clean(&format!(
        "{P}fn main() {{\n    let a = P(x: 1, y: 2)\n    let b = P(x: 1, y: 2)\n    print(a == b)\n}}"
    ));
}

#[test]
fn equal_should_read_every_field_of_an_array_element() {
    judge_unused();
    clean(&format!(
        "{P}fn main() {{\n    let a = P(x: 1, y: 2)\n    let b = P(x: 1, y: 2)\n    print([a] == [b])\n}}"
    ));
}

#[test]
fn equal_should_read_the_fields_of_a_held_class() {
    judge_unused();
    clean(&format!(
        "{P_X}class Q {{\n    p: P\n}}\nfn main() {{\n    let a = Q(p: P(x: 1))\n    print(a == a)\n}}"
    ));
}

#[test]
fn equal_should_read_the_fields_of_a_class_in_a_variant() {
    judge_unused();
    clean(&format!(
        "{P_X}enum E {{\n    Has(P)\n    Empty\n}}\nfn main() {{\n    let e = E.Has(P(x: 1))\n    print(e == E.Empty)\n}}"
    ));
}

#[test]
fn equal_should_read_the_fields_of_a_type_argument() {
    judge_unused();
    clean(&format!(
        "{P_X}class Box<T> {{\n    v: T\n}}\nfn main() {{\n    let b = Box(v: P(x: 1))\n    print(b == b)\n}}"
    ));
}

#[test]
fn equal_should_read_the_fields_of_a_payload_type_argument() {
    judge_unused();
    clean(&format!(
        "{P_X}enum Maybe<T> {{\n    Has(T)\n    Empty\n}}\nfn main() {{\n    let m = Maybe.Has(P(x: 1))\n    print(m == Maybe.Empty)\n}}"
    ));
}

#[test]
fn contains_should_read_every_field() {
    judge_unused();
    clean(&format!(
        "{P_X}fn main() {{\n    let a = P(x: 1)\n    let xs = [a]\n    print(xs.contains(a))\n}}"
    ));
}

#[test]
fn safe_field_read_should_read_the_field() {
    judge_unused();
    clean(&format!(
        "{P_X}fn get(o: P?) -> i64 {{\n    o?.x ?? 0\n}}\nfn main() {{\n    print(get(P(x: 1)))\n}}"
    ));
}

#[test]
fn equal_should_read_no_field_when_it_compares_with_none() {
    judge_unused();
    diagnostic(
        &format!(
            "{P_X}fn show(p: P?) {{\n    print(p == None)\n}}\nfn main() {{\n    show(P(x: 1))\n}}"
        ),
        W0013,
        (2, 5, "x"),
        &[],
    );
}

#[test]
fn contains_should_read_no_field_when_it_searches_for_none() {
    judge_unused();
    diagnostic(
        &format!(
            "{P_X}fn show(xs: [P?]) {{\n    print(xs.contains(None))\n}}\nfn main() {{\n    show([P(x: 1)])\n}}"
        ),
        W0013,
        (2, 5, "x"),
        &[],
    );
}

#[test]
fn field_should_cite_a_dead_comparison() {
    judge_unused();
    assert_eq!(
        found(&format!(
            "{P}fn old_report(p: P) {{\n    print(p == p)\n}}\nfn main() {{\n    print(P(x: 1, y: 2).x)\n}}"
        )),
        [(W0013, (3, 5), vec![(6, 11)]), (W0012, (5, 4), vec![])]
    );
}

#[test]
fn variant_should_warn_when_only_a_pattern_names_it() {
    judge_unused();
    diagnostic(
        &format!(
            "{LIGHT}fn main() {{\n    let l = Light.Green\n    match l {{\n        Red => print(1)\n        Green => print(2)\n    }}\n}}"
        ),
        W0013,
        (2, 5, "Red"),
        &[],
    );
}

#[test]
fn field_should_warn_when_only_a_dead_function_reads_it() {
    judge_unused();
    assert_eq!(
        found(&format!(
            "{P}fn old_report(p: P) {{\n    print(p.y)\n}}\nfn main() {{\n    let p = P(x: 1, y: 2)\n    print(p.x)\n}}"
        )),
        [(W0013, (3, 5), vec![(6, 11)]), (W0012, (5, 4), vec![])]
    );
}

#[test]
fn variant_should_warn_when_only_a_dead_function_builds_it() {
    judge_unused();
    assert_eq!(
        found(&format!(
            "{LIGHT}fn old_report() -> Light {{\n    Light.Red\n}}\nfn main() {{\n    let l = Light.Green\n    print(l == Light.Green)\n}}"
        )),
        [(W0013, (2, 5), vec![(6, 5)]), (W0012, (5, 4), vec![])]
    );
}

#[test]
fn underscore_field_should_not_warn() {
    judge_unused();
    clean(
        "class P {\n    x: i64\n    _pad: i64\n}\nfn main() {\n    let p = P(x: 1, _pad: 0)\n    print(p.x)\n}",
    );
}

#[test]
fn field_should_cite_the_first_dead_comparison_in_source_order() {
    judge_unused();
    // `pick` checks its second argument first, as the first needs its type.
    let src = format!(
        "{P}fn pick<T>(a: T?, b: T) -> T {{\n    a ?? b\n}}\nfn old_report(p: P) {{\n    print(pick(if p == p {{ None }} else {{ None }}, [p] == [p]))\n}}\nfn main() {{\n    print(P(x: 1, y: 2).x)\n    print(pick(None, 1))\n}}"
    );
    assert_eq!(
        found(&src),
        [(W0013, (3, 5), vec![(9, 19)]), (W0012, (8, 4), vec![])]
    );
}

#[test]
fn field_should_cite_a_dead_comparison_only_when_it_has_fewer_than_three_reads() {
    judge_unused();
    // The comparison comes first in the source, so a list of three reads must still keep it out.
    let src = "class P {\n    x: i64\n    y: i64\n    z: i64\n}\nfn a(p: P) {\n    print(p == p)\n}\nfn b(p: P) {\n    print(p.y)\n}\nfn c(p: P) {\n    print(p.y + p.z)\n}\nfn d(p: P) {\n    print(p.y)\n}\nfn main() {\n    print(P(x: 1, y: 2, z: 3).x)\n}";
    assert_eq!(
        found(src),
        [
            (W0013, (3, 5), vec![(10, 11), (13, 11), (16, 11)]),
            (W0013, (4, 5), vec![(7, 11), (13, 17)]),
            (W0012, (6, 4), vec![]),
            (W0012, (9, 4), vec![]),
            (W0012, (12, 4), vec![]),
            (W0012, (15, 4), vec![]),
        ]
    );
}

#[test]
fn variant_should_cite_a_dead_build_of_a_later_variant() {
    judge_unused();
    let src = format!(
        "{LIGHT}fn old_report() -> Light {{\n    Light.Green\n}}\nfn main() {{\n    let l = Light.Red\n    print(l == Light.Red)\n}}"
    );
    assert_eq!(
        found(&src),
        [(W0013, (3, 5), vec![(6, 5)]), (W0012, (5, 4), vec![])]
    );
}

#[test]
fn field_should_cite_each_dead_comparison_when_two_start_at_one_place() {
    judge_unused();
    // The `contains` call and the `==` around it start at one place.
    let src = "class P {\n    x: i64\n}\nclass Q {\n    y: i64\n}\nfn old_report(p: P, q: Q) {\n    print([p].contains(p) == [q].contains(q))\n}\nfn main() {\n    let _ = P(x: 1)\n    let _ = Q(y: 2)\n}";
    assert_eq!(
        found(src),
        [
            (W0013, (2, 5), vec![(8, 11)]),
            (W0013, (5, 5), vec![(8, 30)]),
            (W0012, (7, 4), vec![]),
        ]
    );
}
