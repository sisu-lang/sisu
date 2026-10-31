//! W0012: a function, method, class, enum or constant that no live code uses. W0013: a field
//! that no live code reads, or a variant that no live code builds. W0014: a local or a parameter
//! that nothing reads.

use std::fmt::Write as _;

use super::*;
use crate::check::unused_names;

const W0012: Code = Code::UnusedItem;
const W0013: Code = Code::UnusedMember;
const W0014: Code = Code::UnusedVariable;

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
fn enum_should_hide_its_dead_method_when_nothing_builds_a_variant() {
    judge_unused();
    diagnostic(
        "enum E {\n    A\n    B\n    fn name(self) -> i64 {\n        1\n    }\n}\nfn main() {}",
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

/// The second constant is dead beside a live first one, with no `extend` block and with one: each
/// constant is judged at its own place among the owners of uses.
#[test]
fn constant_should_warn_when_a_later_constant_is_unused_beside_a_live_one() {
    judge_unused();
    diagnostic(
        "const A = 1\nconst B = 2\nfn main() {\n    print(A)\n}",
        W0012,
        (2, 7, "B"),
        &[],
    );
    diagnostic(
        &format!(
            "{SHAPE}{CIRCLE}const A = 1.0\nconst B = 2.0\nfn main() {{\n    print(Circle(r: A).area())\n}}"
        ),
        W0012,
        (11, 7, "B"),
        &[],
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

/// `is_judged` names the warnings that the checker keeps back when a program has an error. The
/// program has a dead function, a field that nothing reads and an unused variable.
#[test]
fn is_judged_should_name_the_warnings_that_an_error_stops() {
    judge_unused();
    let src = "class P {\n    x: i64\n}\nfn dead() {}\nfn main() {\n    let _p = P(x: 1)\n    let u = 1\n}";
    // In order of position: the field, the function, the variable.
    let found: Vec<Code> = diags(src).iter().map(Diagnostic::code).collect();
    assert_eq!(found, [W0013, W0012, W0014], "no error");
    let found = diags(&src.replace("let u = 1", "let u: i64 = \"a\""));
    assert!(
        !found.is_empty() && !found.iter().any(is_judged),
        "{found:?}"
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

/// A comparison reads the fields that the written `eq` reads, and no others: `==` on a class does
/// not read every field, as it did while a class compared its fields.
#[test]
fn field_should_warn_when_the_written_eq_of_a_compared_class_does_not_read_it() {
    judge_unused();
    diagnostic(
        "class P: Eq {\n    x: i64\n    y: i64\n    fn eq(self, other: Self) -> bool { self.x == other.x }\n}\nfn main() {\n    print(P(x: 1, y: 2) == P(x: 1, y: 3))\n}",
        W0013,
        (3, 5, "y"),
        &[],
    );
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

/// `body`, its lines indented, as the body of `f`, which `main` calls.
fn in_f(body: &str) -> String {
    let body: String = body.lines().flat_map(|line| ["    ", line, "\n"]).collect();
    format!("fn f() {{\n{body}}}\nfn main() {{\n    f()\n}}")
}

/// `body`, its lines indented, as the body of `area`, which `main` calls.
fn in_area(body: &str) -> String {
    let body: String = body.lines().flat_map(|line| ["    ", line, "\n"]).collect();
    format!("fn area(w: i64, h: i64) -> i64 {{\n{body}}}\nfn main() {{\n    print(area(1, 2))\n}}")
}

/// Asserts that `src` gives one W0014, at `pos`, with no fix.
fn unread_without_fix(src: &str, pos: (usize, usize, &str)) {
    let d = diagnostic(src, W0014, pos, &[]);
    assert!(fixes(src, &d).is_empty(), "{d:?}");
}

/// Asserts that `src` gives one W0014, at `pos`, whose one safe fix replaces the name with `_`.
fn unread_with_fix(src: &str, pos: (usize, usize, &str)) -> Diagnostic {
    let d = diagnostic(src, W0014, pos, &[]);
    assert_eq!(
        fixes(src, &d),
        [(true, vec![(pos.0, pos.1, pos.2, "_".to_string())])]
    );
    d
}

#[test]
fn let_should_warn_when_nothing_reads_it() {
    judge_unused();
    unread_without_fix(&in_f("let x = 1"), (2, 9, "x"));
}

#[test]
fn pattern_name_should_warn_when_nothing_reads_it() {
    judge_unused();
    unread_with_fix(
        "enum Expr {\n    Add(i64, i64)\n    Num(i64)\n}\nfn f(e: Expr) -> i64 {\n    match e {\n        Add(a, b) => a\n        Num(n) => n\n    }\n}\nfn main() {\n    print(f(Expr.Num(1)) + f(Expr.Add(1, 2)))\n}",
        (7, 16, "b"),
    );
}

#[test]
fn if_let_name_should_warn_when_nothing_reads_it() {
    judge_unused();
    unread_with_fix(
        "fn f(o: i64?) {\n    if let v = o { print(1) }\n}\nfn main() {\n    f(None)\n}",
        (2, 12, "v"),
    );
}

#[test]
fn compound_assign_should_not_read_its_target() {
    judge_unused();
    // `diagnostic` asserts the one diagnostic, so no W0002 either.
    unread_without_fix(&in_f("let mut n = 0\nn += 1"), (2, 13, "n"));
}

#[test]
fn assigned_value_should_read_its_target() {
    judge_unused();
    clean(&in_f("let mut n = 0\nn = n + 1"));
}

#[test]
fn element_write_should_not_read_its_array() {
    judge_unused();
    unread_without_fix(&in_f("let mut xs = [1]\nxs[0] = 2"), (2, 13, "xs"));
}

#[test]
fn field_write_should_read_its_root() {
    judge_unused();
    clean(
        "class P {\n    mut x: i64\n}\nfn f() {\n    let p = P(x: 1)\n    p.x = 2\n}\nfn main() {\n    f()\n    print(P(x: 3).x)\n}",
    );
}

#[test]
fn interpolation_should_read_its_name() {
    judge_unused();
    clean(&in_f("let x = 1\nprint(\"{x}\")"));
}

#[test]
fn underscore_local_should_not_warn() {
    judge_unused();
    clean(&in_f("let _x = 1"));
}

#[test]
fn parameter_should_warn_when_nothing_reads_it() {
    judge_unused();
    unread_without_fix(&in_area("w * w"), (1, 17, "h"));
}

#[test]
fn parameter_should_not_warn_when_its_function_has_todo() {
    judge_unused();
    codes(&in_area("todo"), &[(Code::TodoLeft, (2, 5))]);
}

#[test]
fn local_should_warn_when_its_function_has_todo() {
    judge_unused();
    codes(
        &in_area("let t = 1\ntodo"),
        &[(W0014, (2, 9)), (Code::TodoLeft, (3, 5))],
    );
}

#[test]
fn parameters_should_not_warn_when_both_are_read() {
    judge_unused();
    clean(&in_area("w * h"));
}

#[test]
fn main_parameter_should_warn_when_nothing_reads_it() {
    judge_unused();
    unread_without_fix("fn main(args: [string]) {\n    print(1)\n}", (1, 9, "args"));
}

#[test]
fn underscore_function_should_judge_its_locals() {
    judge_unused();
    unread_without_fix(
        "fn _old_report() {\n    let t = 1\n}\nfn main() {}",
        (2, 9, "t"),
    );
}

#[test]
fn local_should_not_warn_when_its_function_is_dead() {
    judge_unused();
    codes(
        "fn old_report() {\n    let x = 1\n}\nfn main() {}",
        &[(W0012, (1, 4))],
    );
}

#[test]
fn never_reassigned_should_warn_when_its_function_is_dead() {
    judge_unused();
    codes(
        "fn old_report() {\n    let mut x = 1\n}\nfn main() {}",
        &[(W0012, (1, 4)), (Code::NeverReassigned, (2, 13))],
    );
}

#[test]
fn never_reassigned_should_warn_when_the_program_has_an_error() {
    judge_unused();
    codes(
        "fn helper() {\n    print(1)\n}\nfn main() {\n    let mut m = 1\n    let u: i64 = \"a\"\n}",
        &[(Code::NeverReassigned, (5, 13)), (Code::Mismatch, (6, 18))],
    );
}

#[test]
fn loop_condition_should_read_its_local() {
    judge_unused();
    codes(
        "fn main() {\n    let i = 0\n    while i < 3 { print(1) }\n}",
        &[(Code::UnchangedLoopCondition, (3, 11))],
    );
}

#[test]
fn for_fix_should_leave_a_clean_program() {
    judge_unused();
    let src = in_f("for i in 0..3 { print(1) }");
    let d = unread_with_fix(&src, (2, 9, "i"));
    clean(&applied(&src, &d));
}

#[test]
fn never_reassigned_should_warn_when_its_binding_is_read() {
    judge_unused();
    codes(
        &in_f("let mut k = 1\nprint(k)"),
        &[(Code::NeverReassigned, (2, 13))],
    );
}

#[test]
fn shadowed_local_should_warn_when_nothing_reads_it() {
    judge_unused();
    unread_without_fix(&in_f("let x = 1\nlet x = 2\nprint(x)"), (2, 9, "x"));
}

#[test]
fn or_pattern_fix_should_edit_every_alternative() {
    judge_unused();
    let src = "enum Op {\n    Add(i64, i64)\n    Sub(i64, i64)\n}\nfn f(o: Op) -> i64 {\n    match o {\n        Add(a, b) | Sub(a, b) => a\n    }\n}\nfn main() {\n    print(f(Op.Add(1, 2)) + f(Op.Sub(3, 4)))\n}";
    let d = diagnostic(src, W0014, (7, 16, "b"), &[]);
    assert_eq!(
        fixes(src, &d),
        [(
            true,
            vec![(7, 16, "b", "_".to_string()), (7, 28, "b", "_".to_string())]
        )]
    );
    clean(&applied(src, &d));
}

#[test]
fn compound_assign_should_read_its_value_when_it_names_the_target() {
    judge_unused();
    clean(&in_f("let mut n = 1\nn += n"));
}

#[test]
fn never_reassigned_should_warn_once_when_the_checker_checks_it_twice() {
    judge_unused();
    // An expected `u8` makes the checker check the join, and so the block, again.
    codes(
        "fn main() {\n    let s: u8 = \"a\" + if true {\n        let mut t = 1\n        \"b\"\n    } else {\n        \"c\"\n    }\n}",
        &[(Code::Mismatch, (2, 17)), (Code::NeverReassigned, (3, 17))],
    );
}

/// An interface, and the code that follows.
const SHAPE: &str = "interface Shape {\n    fn area(self) -> f64\n}\n";

/// A method of a conformance is live when its class or enum is built: an interface call names a
/// requirement, so no use of the method itself reaches it, as rustc reports no method of a trait
/// impl of a type it builds.
#[test]
fn conformance_method_should_not_warn_when_its_type_is_built() {
    judge_unused();
    for src in [
        // In the list of a class, in a block of a class, and in a block of an enum.
        "class Circle: Shape {\n    r: f64\n    fn area(self) -> f64 { self.r }\n}\nfn main() {\n    print(Circle(r: 1.0).r)\n}",
        "class Circle {\n    r: f64\n}\nextend Circle: Shape {\n    fn area(self) -> f64 { self.r }\n}\nfn main() {\n    print(Circle(r: 1.0).r)\n}",
        "enum Dir {\n    North\n}\nextend Dir: Shape {\n    fn area(self) -> f64 { 1.0 }\n}\nfn main() {\n    let _ = Dir.North\n}",
        "enum Dir: Shape {\n    North\n    fn area(self) -> f64 { 1.0 }\n}\nfn main() {\n    let _ = Dir.North\n}",
    ] {
        // Nothing calls the method or names the interface, so the interface is the one warning:
        // the method of a conformance of a type that is built gives none of its own.
        assert_eq!(
            found(&format!("{SHAPE}{src}")),
            [(W0012, (1, 11), vec![])],
            "{src}"
        );
    }
}

/// A conformance method that live code calls before its class is built is live already.
#[test]
fn conformance_method_should_not_warn_when_a_root_calls_it_before_its_class_is_built() {
    judge_unused();
    clean(&format!(
        "{SHAPE}fn main() {{\n    print(Circle(r: 1.0).r)\n}}\nfn _show(c: Circle) -> f64 {{\n    c.area()\n}}\nclass Circle: Shape {{\n    r: f64\n    fn area(self) -> f64 {{ self.r }}\n}}"
    ));
}

/// What only the method of a conformance of an unbuilt class calls is not live: the method is
/// not, so rustc reports the helper, and the class is reported, not its method.
#[test]
fn helper_should_warn_when_only_the_conformance_method_of_an_unbuilt_class_calls_it() {
    judge_unused();
    let src = format!(
        "{SHAPE}fn helper() -> f64 {{\n    1.0\n}}\nclass Circle: Shape {{\n    r: f64\n    fn area(self) -> f64 {{ helper() }}\n}}\nfn main() {{}}"
    );
    assert_eq!(
        found(&src),
        [
            (W0012, (1, 11), vec![]),
            (W0012, (4, 4), vec![(9, 28)]),
            (W0012, (7, 7), vec![])
        ]
    );
}

/// A block with no live method is unused when its type is built: the block is reported, and its
/// methods are not. An unbuilt type is the one reported. A call is a use, whichever block holds
/// the method.
#[test]
fn extend_block_should_warn_only_when_its_type_is_built() {
    judge_unused();
    let block = "extend Circle {\n    fn grow(self) -> f64 { 1.0 }\n}\n";
    let class = "class Circle {\n    r: f64\n}\n";
    diagnostic(
        &format!("{class}{block}fn main() {{\n    print(Circle(r: 1.0).r)\n}}"),
        W0012,
        (4, 1, "extend Circle"),
        &[],
    );
    diagnostic(
        &format!("{class}{block}fn main() {{}}"),
        W0012,
        (1, 7, "Circle"),
        &[],
    );
    clean(&format!(
        "{class}{block}fn main() {{\n    let c = Circle(r: 1.0)\n    print(c.grow() + c.r)\n}}"
    ));
}

/// A generic class `Crate`, and a block of it with a bound, a live method `live` and a dead method
/// `spare`, and a `main` that calls `live` on a `Circle`.
const CRATE_T: &str = "class Crate<T> {\n    inner: T\n}\n";
const LIVE_BLOCK: &str = "extend<T: Shape> Crate<T> {\n    fn live(self) -> f64 { self.inner.area() }\n    fn spare(self) -> f64 { 1.0 }\n}\n";
const CALLS_LIVE: &str = "fn main() {\n    print(Crate(inner: Circle(r: 1.0)).live())\n}";

/// A block with a live method is live, so a dead method of it is reported by itself.
#[test]
fn extend_block_should_be_live_when_one_of_its_methods_is_live() {
    judge_unused();
    diagnostic(
        &format!("{SHAPE}{CIRCLE}{CRATE_T}{LIVE_BLOCK}{CALLS_LIVE}"),
        W0012,
        (15, 8, "spare"),
        &[],
    );
}

/// The second block is dead and the first is live: only the second is reported, beside the dead
/// method of the first.
#[test]
fn extend_blocks_of_one_type_should_be_judged_one_by_one() {
    judge_unused();
    assert_eq!(
        found(&format!(
            "{SHAPE}{CIRCLE}{CRATE_T}{LIVE_BLOCK}extend<T> Crate<T> {{\n    fn other(self) -> i64 {{ 1 }}\n}}\n{CALLS_LIVE}"
        )),
        [(W0012, (15, 8), vec![]), (W0012, (17, 1), vec![])]
    );
}

/// An enum with a dead block is judged as a class is: the block is reported, and the interface
/// that only it names.
#[test]
fn extend_block_of_an_enum_should_be_judged_as_one_of_a_class_is() {
    judge_unused();
    let maybe = "enum Maybe<T> {\n    Has(T)\n    No\n}\nextend<T: Shape> Maybe<T> {\n    fn spare(self) -> f64 { 1.0 }\n}\n";
    assert_eq!(
        found(&format!(
            "{SHAPE}{maybe}fn main() {{\n    let m: Maybe<i64> = Maybe.Has(1)\n    print(m == Maybe.No)\n}}"
        )),
        [(W0012, (1, 11), vec![(8, 11)]), (W0012, (8, 1), vec![])]
    );
}

/// A name that starts with `_` keeps its block live, so only the dead method beside it is reported.
#[test]
fn extend_block_should_be_live_when_one_of_its_methods_starts_with_underscore() {
    judge_unused();
    diagnostic(
        "class Crate {\n    n: i64\n}\nextend Crate {\n    fn _keep(self) -> i64 { 1 }\n    fn spare(self) -> i64 { 2 }\n}\nfn main() {\n    print(Crate(n: 1).n)\n}",
        W0012,
        (6, 8, "spare"),
        &[],
    );
}

/// A call of a method of a dead block in dead code is a use of the block: the warning cites it,
/// whether the caller is a function or a method of another dead block. A call from inside the
/// block itself is not cited.
#[test]
fn extend_block_should_cite_the_calls_of_its_methods_in_dead_code() {
    judge_unused();
    let class = "class Crate {\n    n: i64\n}\n";
    let main = "fn main() {\n    print(Crate(n: 1).n)\n}";
    assert_eq!(
        found(&format!(
            "{class}extend Crate {{\n    fn grow(self) -> i64 {{ self.n + 1 }}\n}}\nfn unused(c: Crate) -> i64 {{\n    c.grow()\n}}\n{main}"
        )),
        [(W0012, (4, 1), vec![(8, 5)]), (W0012, (7, 4), vec![])]
    );
    assert_eq!(
        found(&format!(
            "{class}extend Crate {{\n    fn a(self) -> i64 {{ self.b() }}\n}}\nextend Crate {{\n    fn b(self) -> i64 {{ self.a() }}\n}}\n{main}"
        )),
        [
            (W0012, (4, 1), vec![(8, 25)]),
            (W0012, (7, 1), vec![(5, 25)])
        ]
    );
    assert_eq!(
        found(&format!(
            "{class}extend Crate {{\n    fn a(self) -> i64 {{ self.b() }}\n    fn b(self) -> i64 {{ 1 }}\n}}\n{main}"
        )),
        [(W0012, (4, 1), vec![])]
    );
}

/// Each block is judged by its own place among the blocks: the third of three is live, and the
/// two before it are not, and the bounds of two dead blocks are two uses of one interface.
#[test]
fn extend_block_should_be_judged_by_its_own_place_among_blocks() {
    judge_unused();
    let crate_ = "class Crate<T> {\n    inner: T\n}\n";
    let dead =
        |name: &str| format!("extend<T> Crate<T> {{\n    fn {name}(self) -> i64 {{ 1 }}\n}}\n");
    let live = "extend<T: Shape> Crate<T> {\n    fn live(self) -> f64 { self.inner.area() }\n}\n";
    let main = "fn main() {\n    print(Crate(inner: Circle(r: 1.0)).live())\n}";
    assert_eq!(
        found(&format!(
            "{SHAPE}{CIRCLE}{crate_}{}{}{live}{main}",
            dead("first"),
            dead("second")
        )),
        [(W0012, (13, 1), vec![]), (W0012, (16, 1), vec![])]
    );
    let bound = |name: &str| {
        format!("extend<T: Shape> Crate<T> {{\n    fn {name}(self) -> i64 {{ 1 }}\n}}\n")
    };
    assert_eq!(
        found(&format!(
            "{SHAPE}{crate_}{}{}fn main() {{\n    print(Crate(inner: 1).inner)\n}}",
            bound("first"),
            bound("second")
        )),
        [
            (W0012, (1, 11), vec![(7, 11), (10, 11)]),
            (W0012, (7, 1), vec![]),
            (W0012, (10, 1), vec![])
        ]
    );
}

/// A class that conforms to `Shape` with an `area` that nothing calls, and `main` that builds it.
const CIRCLE: &str = "class Circle {\n    r: f64\n}\nextend Circle: Shape {\n    fn area(self) -> f64 { self.r }\n}\n";

/// An interface that nothing names is unused, and so is one that only a conformance names: a
/// conformance dispatches nothing. The methods of the conformance of a type that is built are live,
/// so they give no warning of their own. A bound in dead code names the interface for no live
/// code, and the warning cites it.
#[test]
fn interface_should_warn_when_no_live_code_names_or_calls_it() {
    judge_unused();
    diagnostic(
        &format!("{SHAPE}fn main() {{}}"),
        W0012,
        (1, 11, "Shape"),
        &[],
    );
    // Only `extend Circle: Shape` names it, and `Circle` is built.
    diagnostic(
        &format!("{SHAPE}{CIRCLE}fn main() {{\n    print(Circle(r: 1.0).r)\n}}"),
        W0012,
        (1, 11, "Shape"),
        &[],
    );
    // Only a dead function's bound names it: the warning cites the bound.
    assert_eq!(
        found(&format!(
            "{SHAPE}fn dead<T: Shape>(x: T) -> T {{ x }}\nfn main() {{}}"
        )),
        [(W0012, (1, 11), vec![(4, 12)]), (W0012, (4, 4), vec![])]
    );
    // The block of a type that is built and whose methods are all dead is reported. The bound
    // is one use of the interface, not one for each method.
    assert_eq!(
        found(&format!(
            "{SHAPE}class Crate<T> {{\n    inner: T\n}}\nextend<T: Shape> Crate<T> {{\n    fn spelled(self) -> i64 {{ 1 }}\n    fn second(self) -> i64 {{ 2 }}\n}}\nfn main() {{\n    print(Crate(inner: 1).inner)\n}}"
        )),
        [(W0012, (1, 11), vec![(7, 11)]), (W0012, (7, 1), vec![])]
    );
    // The same for a block with no method.
    assert_eq!(
        found(&format!(
            "{SHAPE}class Crate<T> {{\n    inner: T\n}}\nextend<T: Shape> Crate<T> {{\n}}\nfn main() {{\n    print(Crate(inner: 1).inner)\n}}"
        )),
        [(W0012, (1, 11), vec![(7, 11)]), (W0012, (7, 1), vec![])]
    );
    // The class is never built, so it is the one reported, and the bound is one use.
    assert_eq!(
        found(&format!(
            "{SHAPE}class Crate<T> {{\n    inner: T\n}}\nextend<T: Shape> Crate<T> {{\n    fn one(self) -> i64 {{ 1 }}\n    fn two(self) -> i64 {{ 2 }}\n}}\nfn main() {{}}"
        )),
        [(W0012, (1, 11), vec![(7, 11)]), (W0012, (4, 7), vec![])]
    );
    assert_eq!(
        found(&format!(
            "{SHAPE}class Crate<T> {{\n    inner: T\n}}\nextend<T: Shape> Crate<T> {{\n}}\nfn main() {{}}"
        )),
        [(W0012, (1, 11), vec![(7, 11)]), (W0012, (4, 7), vec![])]
    );
    // A conformance block names the interface in dead code only: its class is built in dead code.
    assert_eq!(
        found(&format!(
            "{SHAPE}interface Marker {{\n}}\nclass Crate<T> {{\n    inner: T\n}}\nextend<T: Shape> Crate<T>: Marker {{\n}}\nfn unused() {{\n    let c = Crate(inner: 1)\n    print(c.inner)\n}}\nfn main() {{}}"
        )),
        [
            (W0012, (1, 11), vec![(9, 11)]),
            (W0012, (4, 11), vec![]),
            (W0012, (6, 7), vec![(12, 13)]),
            (W0012, (11, 4), vec![])
        ]
    );
}

/// An interface that live code names in a bound, calls through a requirement or a method of a
/// conformance, or that a used interface names, is used.
#[test]
fn interface_should_not_warn_when_live_code_uses_it() {
    judge_unused();
    let circle = |rest: &str| format!("{SHAPE}{CIRCLE}{rest}");
    for src in [
        // (a) A live function's bound, a live method's bound, and a built class's bound.
        circle("fn total<T: Shape>(_x: T) -> f64 {\n    1.0\n}\nfn main() {\n    print(total(Circle(r: 1.0)))\n}"),
        circle("extend Circle {\n    fn fit<T: Shape>(self, _x: T) -> f64 { 1.0 }\n}\nfn main() {\n    let c = Circle(r: 1.0)\n    print(c.fit(c))\n}"),
        circle("class Sorted<T: Shape> {\n    items: [T]\n}\nfn main() {\n    let s: Sorted<Circle> = Sorted(items: [Circle(r: 1.0)])\n    print(s.items.len())\n}"),
        // (a) A live method's block names it in a bound of the block's parameter: for a plain
        // block, for a conformance block, and beside the bound of a generic method.
        circle("class Crate<T> {\n    inner: T\n}\nextend<T: Shape> Crate<T> {\n    fn spelled(self) -> f64 { self.inner.area() }\n}\nfn main() {\n    print(Crate(inner: Circle(r: 1.0)).spelled())\n}"),
        circle("interface Other {\n    fn other(self) -> f64\n}\nclass Crate<T> {\n    inner: T\n}\nextend<T: Shape> Crate<T>: Other {\n    fn other(self) -> f64 { self.inner.area() }\n}\nfn main() {\n    print(Crate(inner: Circle(r: 1.0)).other())\n}"),
        circle("class Crate<T> {\n    inner: T\n}\nextend<T> Crate<T> {\n    fn spelled<U: Shape>(self, _u: U) -> i64 { 3 }\n}\nfn main() {\n    let c = Crate(inner: 1)\n    print(c.inner)\n    print(c.spelled(Circle(r: 1.0)))\n}"),
        // (a) The bound of a block with no live method names it when the block conforms a built
        // class or enum: no method names the bound.
        circle("interface Marker {\n}\nclass Crate<T> {\n    inner: T\n}\nextend<T: Shape> Crate<T>: Marker {\n}\nfn want<X: Marker>(_x: X) -> i64 {\n    2\n}\nfn main() {\n    let c = Crate(inner: Circle(r: 1.0))\n    print(c.inner.r)\n    print(want(c))\n}"),
        circle("interface Marker {\n}\nenum Maybe<T> {\n    Has(T)\n    No\n}\nextend<T: Shape> Maybe<T>: Marker {\n}\nfn want<X: Marker>(_x: X) -> i64 {\n    2\n}\nfn main() {\n    let m: Maybe<Circle> = Maybe.No\n    print(want(m))\n    print(want(Maybe.Has(Circle(r: 1.0))))\n}"),
        // (b) A method of a conformance, with no bound: the call selects it.
        circle("fn main() {\n    print(Circle(r: 1.0).area())\n}"),
        // A method of the body that meets a requirement of a listed interface.
        format!("{SHAPE}class Disc: Shape {{\n    r: f64\n    fn area(self) -> f64 {{ self.r }}\n}}\nfn main() {{\n    print(Disc(r: 1.0).area())\n}}"),
        // (c) An interface that a used interface inherits.
        "interface Named {\n    fn name(self) -> string\n}\ninterface Shape: Named {\n    fn area(self) -> f64\n}\nclass Disc: Shape {\n    fn name(self) -> string { \"d\" }\n    fn area(self) -> f64 { 1.0 }\n}\nfn total<T: Shape>(_x: T) -> f64 {\n    1.0\n}\nfn main() {\n    print(total(Disc()))\n}".to_string(),
        // (c) An interface that the bound of a used requirement's own parameter names.
        "interface Same {\n    fn same(self, other: Self) -> bool\n}\ninterface Mapper {\n    fn map<U: Same>(self, x: U) -> U\n}\nclass Id: Mapper {\n    fn map<U: Same>(self, x: U) -> U { x }\n}\nfn use_mapper<T: Mapper>(_m: T) -> i64 {\n    1\n}\nfn main() {\n    print(use_mapper(Id()))\n}".to_string(),
        // A name that starts with `_` is kept on purpose.
        "interface _Shape {\n    fn area(self) -> f64\n}\nfn main() {}".to_string(),
    ] {
        clean(&src);
    }
}

/// A generic interface is used by a call that selects a method of its conformance.
#[test]
fn generic_interface_should_not_warn_when_a_call_selects_its_conformance() {
    judge_unused();
    for src in [
        // The program of the language specification.
        "interface Convert<T> {\n    fn convert(self) -> T\n}\nclass Fahrenheit {\n    d: f64\n}\nclass Celsius {\n    d: f64\n}\nextend Celsius: Convert<Fahrenheit> {\n    fn convert(self) -> Fahrenheit { Fahrenheit(d: self.d) }\n}\nfn main() {\n    let f: Fahrenheit = Celsius(d: 1.0).convert()\n    print(f.d)\n}",
        // The `Transform` example of the generics reference.
        "class Celsius {\n    degrees: f64\n}\nclass Fahrenheit {\n    degrees: f64\n}\ninterface Transform<T> {\n    fn transform(self) -> T\n}\nextend Celsius: Transform<Fahrenheit> {\n    fn transform(self) -> Fahrenheit {\n        Fahrenheit(degrees: self.degrees * 1.8 + 32.0)\n    }\n}\nfn main() {\n    let f: Fahrenheit = Celsius(degrees: 100.0).transform()\n    print(f.degrees)\n}",
    ] {
        clean(src);
    }
}

/// An interface that only the bound of a used interface's own type parameter names is used: the
/// conformance that meets the bound names it too, and a conformance is no use.
#[test]
fn interface_should_not_warn_when_the_bound_of_a_used_interfaces_parameter_names_it() {
    judge_unused();
    clean(
        "interface Same {\n    fn same(self, other: Self) -> bool\n}\ninterface Feed<T: Same> {\n    fn feed(self, x: T) -> i64\n}\nclass A {\n    n: i64\n}\nextend A: Same {\n    fn same(self, other: A) -> bool { self.n == other.n }\n}\nclass B {\n    m: i64\n}\nextend B: Feed<A> {\n    fn feed(self, x: A) -> i64 { self.m + x.n }\n}\nfn run<T: Feed<A>>(x: T, a: A) -> i64 {\n    x.feed(a)\n}\nfn main() {\n    print(run(B(m: 1), A(n: 2)))\n}",
    );
}

/// An interface that only the bound of a used interface's associated type names is used: the
/// class that is the item conforms to it, and a conformance is no use.
#[test]
fn interface_should_not_warn_when_the_bound_of_a_used_interfaces_associated_type_names_it() {
    judge_unused();
    clean(
        "interface Same {\n    fn same(self, other: Self) -> bool\n}\ninterface Source {\n    type Item: Same\n    fn next(self) -> Item?\n}\nclass Tag: Same {\n    id: i64\n    fn same(self, other: Tag) -> bool { self.id == other.id }\n}\nclass Countdown: Source {\n    type Item = Tag\n    n: i64\n    fn next(self) -> Item? { if self.n > 0 { Tag(id: self.n) } else { None } }\n}\nfn first_of<S: Source>(s: S) -> S.Item? {\n    s.next()\n}\nfn main() {\n    print(first_of(Countdown(n: 1)) == None)\n    print(Tag(id: 1).id)\n}",
    );
}

/// An interface of the prelude is out of the user's sight: nothing in it is unused, as nothing
/// else in the prelude is.
#[test]
fn prelude_interface_should_not_warn_when_nothing_uses_it() {
    judge_unused();
    let prelude = prelude_program("interface Stock {\n    fn stock(self) -> i64\n}\n");
    let src = "fn main() {}";
    let (_, ds) = check(&parse_ok(src), &prelude, src, &[]);
    assert!(ds.is_empty(), "{ds:?}");
}

/// A write to a field does not read it, and nor does a `push` onto it.
#[test]
fn field_should_warn_when_live_code_only_pushes_onto_it() {
    judge_unused();
    diagnostic(
        "class P {\n    mut xs: [i64]\n}\nfn main() {\n    let p = P(xs: [1])\n    p.xs.push(2)\n}",
        W0013,
        (2, 9, "xs"),
        &[],
    );
}

/// A `push` onto an element names the field of the array without reading it. It reads the fields
/// of the index. So `b` is only pushed onto and warns, and `c` is read and does not.
#[test]
fn field_should_warn_when_live_code_only_pushes_onto_one_of_its_elements() {
    judge_unused();
    diagnostic(
        "class P {\n    mut b: [[i64]]\n    c: i64\n}\nfn main() {\n    let p = P(b: [[1]], c: 0)\n    p.b[p.c].push(2)\n}",
        W0013,
        (2, 9, "b"),
        &[],
    );
}

/// A method named `push` of a class reads its receiver as any method does: the local, and the
/// field that holds the object, whether it is reached by an index or not, are used.
#[test]
fn receiver_of_a_class_push_should_be_read() {
    judge_unused();
    let src = "class Pile {\n    mut n: i64\n    fn push(self, x: i64) { self.n += x }\n}\nclass Holder {\n    pile: Pile\n    piles: [Pile]\n}\nfn main() {\n    let pile = Pile(n: 0)\n    pile.push(1)\n    let holder = Holder(pile: pile, piles: [pile])\n    holder.pile.push(2)\n    holder.piles[0].push(3)\n    let piles = [pile]\n    piles[0].push(4)\n    print(holder.pile.n)\n}";
    assert_eq!(found(src), Vec::new());
}

/// How many types the W0013 walk of `dbg` read for `src`, which holds a field that nothing reads.
fn shown_visits(src: &str) -> usize {
    judge_unused();
    unused_names::SHOWN_VISITS.set(0);
    let ds = diags(src);
    assert!(
        ds.iter().any(|d| d.code() == W0013),
        "the program should hold a field that nothing reads: {ds:?}"
    );
    unused_names::SHOWN_VISITS.get()
}

/// The chain `E0<T>` to `E{n-1}<T>`: `Ek` has the variants `A([E(k+1)<[T]>])` and
/// `B(E(k+1)<T?>)`, so the instances of the next enum double at each level. The last enum holds
/// the class `P` and the parameter. `main` builds `E0<i64>` and shows it with `dbg`.
fn enum_chain(n: usize) -> String {
    let mut src = String::from("class P {\n    x: i64\n}\n");
    for k in 0..n - 1 {
        let next = k + 1;
        writeln!(
            src,
            "enum E{k}<T> {{\n    A([E{next}<[T]>])\n    B(E{next}<T?>)\n    Z\n}}"
        )
        .expect("writing to a `String` should succeed");
    }
    let last = n - 1;
    writeln!(src, "enum E{last}<T> {{\n    A(P)\n    B(T)\n}}")
        .expect("writing to a `String` should succeed");
    src + "fn main() {\n    let _p = P(x: 1)\n    let e: E0<i64> = E0.Z\n    dbg(e)\n}"
}

/// The chain `C0<T>` to `C{n-1}<T>` of classes, as `enum_chain` is of enums.
fn class_chain(n: usize) -> String {
    let mut src = String::new();
    for k in 0..n - 1 {
        let next = k + 1;
        writeln!(
            src,
            "class C{k}<T> {{\n    a: [C{next}<[T]>]\n    b: [C{next}<T?>]\n}}"
        )
        .expect("writing to a `String` should succeed");
    }
    let last = n - 1;
    writeln!(src, "class C{last}<T> {{\n    x: T\n}}")
        .expect("writing to a `String` should succeed");
    src + "fn main() {\n    dbg(C0<i64>(a: [], b: []))\n}"
}

/// The walk reads the structure of the operand and each enum declaration once, not the instances
/// that the declarations make, which double at each level of the chain.
#[test]
fn shown_by_dbg_should_visit_each_enum_declaration_once() {
    let (short, long) = (6, 12);
    let (few, many) = (
        shown_visits(&enum_chain(short)),
        shown_visits(&enum_chain(long)),
    );
    assert!(
        many - few <= 4 * (long - short),
        "{few} types for {short} enums and {many} types for {long} enums"
    );
    assert!(many > few, "each enum should be read: {few} and {many}");
}

/// A class shows its fields as `print` does, so the walk stops at it: it reads no type that the
/// class holds, whatever the chain of classes below it.
#[test]
fn shown_by_dbg_should_stop_at_a_class() {
    assert_eq!(shown_visits(&class_chain(12)), 1);
    assert_eq!(shown_visits(&class_chain(6)), 1);
}

/// A `dbg` that no live code reaches gives no label, and does not take the label from a later `dbg`
/// that live code reaches.
#[test]
fn shown_by_dbg_should_ignore_a_dbg_in_dead_code() {
    judge_unused();
    let dead = "class P {\n    x: i64\n}\nfn never(p: P) {\n    dbg(p)\n}\n";
    assert_eq!(
        found(&format!(
            "{dead}fn main() {{\n    let p = P(x: 1)\n    print(p)\n}}"
        )),
        [(W0013, (2, 5), vec![]), (W0012, (4, 4), vec![])]
    );
    assert_eq!(
        found(&format!(
            "{dead}fn main() {{\n    let p = P(x: 1)\n    dbg(p)\n}}"
        )),
        [(W0013, (2, 5), vec![(9, 5)]), (W0012, (4, 4), vec![])]
    );
}

/// `dbg` shows the fields of a class that its operand is, or holds in an array, an optional or an
/// enum payload. It writes a class field as `print` does, and a written `to_string` as its text.
#[test]
fn shown_by_dbg_should_label_only_the_fields_that_dbg_shows() {
    judge_unused();
    let row = |decls: &str, shown: &str| {
        let src = format!("{decls}fn main() {{\n    {shown}\n}}");
        found(&src)
            .into_iter()
            .filter(|(code, ..)| *code == W0013)
            .map(|(_, at, shows)| (at.0, !shows.is_empty()))
            .collect::<Vec<_>>()
    };
    let p = "class P {\n    x: i64\n}\n";
    let q = format!("{p}class Q {{\n    p: P\n}}\n");
    // Line 2 is `x`, and line 5 is `p`.
    assert_eq!(row(p, "dbg(P(x: 1))"), [(2, true)], "the operand");
    assert_eq!(
        row(&q, "dbg(Q(p: P(x: 1)))"),
        [(2, false), (5, true)],
        "a class field"
    );
    assert_eq!(row(p, "dbg([P(x: 2)])"), [(2, true)], "an array");
    let written =
        "class R {\n    y: i64\n    fn to_string(self) -> string {\n        \"r\"\n    }\n}\n";
    assert_eq!(
        row(written, "dbg(R(y: 2))"),
        [(2, false)],
        "a written to_string"
    );
    assert_eq!(row(written, "dbg([R(y: 2)])"), [(2, false)], "in an array");
    let boxed = "class Box<T> {\n    v: T\n}\n";
    assert_eq!(
        row(&format!("{p}{boxed}"), "dbg(Box(v: P(x: 1)))"),
        [(2, false), (5, true)],
        "a generic class"
    );
}
