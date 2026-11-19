//! Arrays: literals and their element join, `[v; n]`, indexing, `len()`, `sum()`,
//! `contains()`, and the rules that keep arrays values.

use super::*;

const NODE: &str = "class Node {\n    v: i64\n}\n";

/// `src` after the `Node` class, with an empty `main`.
fn with_node(src: &str) -> String {
    format!("{NODE}{src}\nfn main() {{}}")
}

#[test]
fn element_join() {
    contains(
        "fn main() { let xs = [None, 1] }",
        &["(array None (wrap 1))"],
    );
    contains(
        "fn main() { let g = [[], [1]] }",
        &["(array (array) (array 1))"],
    );
    contains(
        "fn f(b: u8) { let a = [b, 1] }\nfn main() {}",
        &["(array b#0 1u8)"],
    );
    contains(
        "fn f(m: i64?, n: i64) {\n    let a = [m, n]\n    let b = [n, m]\n}\nfn main() {}",
        &["(array m#0 (wrap n#1))", "(array (wrap n#1) m#0)"],
    );
}

#[test]
fn expected_type_flows_into_elements() {
    contains(
        &with_node("fn f(n: Node) { let a: [Node?] = [n, n] }"),
        &["(array (wrap n#0) (wrap n#0))"],
    );
    contains(
        "fn main() { let bytes: [u8?] = [1, None] }",
        &["(array (wrap 1u8) None)"],
    );
}

#[test]
fn fill() {
    contains(
        "fn f(n: i64) { let a = [0; n] }\nfn main() {}",
        &["(array-fill 0 n#0)"],
    );
    contains(
        "fn main() {\n    let b: [u8] = [0; 4]\n    let a: [i64?] = [5; 2]\n}",
        &["(array-fill 0u8 4)", "(array-fill (wrap 5) 2)"],
    );
}

#[test]
fn fill_allows_none_and_arrays_of_classes() {
    clean(&with_node("fn f() { let xs: [Node?] = [None; 3] }"));
    clean(&with_node("fn f(n: Node) { let a = [[n]; 3] }"));
}

#[test]
fn empty_array_typed_by_sibling_branch() {
    contains(
        "fn f(c: bool, ys: [i64]) {\n    let a = if c { [] } else { ys }\n    let b = if c { ys } else { [] }\n}\nfn main() {}",
        &[
            "(let a#2 (if c#0 (block (array)) (block ys#1)))",
            "(let b#3 (if c#0 (block ys#1) (block (array))))",
        ],
    );
    contains(
        "fn f(c: bool, xs: [i64]) -> [i64] { if c { xs } else { [] } }\nfn main() {}",
        &["(block (array))"],
    );
}

#[test]
fn index_and_len() {
    contains(
        "fn f(xs: [[i64]], i: i64) -> i64 { xs[i][0] }\nfn main() {}",
        &["(index (index xs#0 i#1) 0)"],
    );
    contains(
        "fn f(m: [i64]?) -> i64? { m?.len() }\nfn main() {}",
        &["(len #1)"],
    );
}

#[test]
fn equality_wraps_the_plain_side() {
    contains(
        "fn f(a: [i64], b: [i64]?) -> bool { a == b }\nfn main() {}",
        &["(== (wrap a#0) b#1)"],
    );
}

#[test]
fn operands_spill_before_an_exit() {
    contains(
        "fn make() -> [i64] { [1] }\nfn f(c: bool) -> i64 { make()[if c { return 0 } else { 1 }] }\nfn main() {}",
        &[
            "(block (let #1 (call make)) (let #2 (if c#0 (block (return 0)) (block 1))) (index #1 #2))",
        ],
    );
    contains(
        "fn f(c: bool) -> i64 {\n    let g = [[1], if c { return 0 } else { [2] }]\n    0\n}\nfn main() {}",
        &[
            "(block (let #1 (array 1)) (let #2 (if c#0 (block (return 0)) (block (array 2)))) (array #1 #2))",
        ],
    );
    contains(
        "fn f(c: bool) -> i64 {\n    let g = [[1]; if c { return 0 } else { 3 }]\n    0\n}\nfn main() {}",
        &[
            "(block (let #1 (array 1)) (let #2 (if c#0 (block (return 0)) (block 3))) (array-fill #1 #2))",
        ],
    );
}

/// A literal whose expected type failed still types and joins its elements: only what takes
/// its type from the context is left quiet. Some mismatches between its elements wait for the next
/// run, and others are still reported: `tests/expectation.rs` holds the cases. Each quiet row
/// checks clean with an array type written for `Bad`.
#[test]
fn a_literal_in_a_failed_context_should_still_join_its_elements() {
    let quiet = [
        "[[None; 1]]",
        "[[None; 1]; 2]",
        "[[[]]]",
        "[loop { break [] }]",
        "[[loop { break [] }]]",
        "[make(1)]",
        "[[make(1)]]",
        "[id([])]",
        "[if true { loop { break [] } } else { [1] }]",
        "[{ [] }]",
        "[match true { _ => None }]",
        // The `else` block's value is a parenthesized `if`, which shares its span.
        "[if true { [] } else { (if false { id([]) } else { [] }) }]",
    ];
    for literal in quiet {
        let src = format!(
            "fn main() {{\n    let xs: Bad = {literal}\n}}\nfn id<T>(x: T) -> T {{ x }}\nfn make<T>(n: i64) -> [T] {{ [] }}"
        );
        codes(&src, &[(Code::UnknownType, (2, 13))]);
    }
    for (src, expected) in [
        // A statement does not take the literal's type, so its own failure is reported.
        (
            "fn main() {\n    let xs: Bad = [{\n        let q = []\n        1\n    }]\n}",
            vec![(Code::UnknownType, (2, 13)), (Code::CannotInfer, (3, 17))],
        ),
        (
            "fn main() {\n    let xs: Bad = [nope, [None; 1]]\n}",
            vec![
                (Code::UnknownType, (2, 13)),
                (Code::BindingNotFound, (2, 20)),
            ],
        ),
        // The elements that no sibling hints still join: a `string` and a `bool` do not.
        (
            "fn main() {\n    let xs: Bad = [\"a\", true]\n}",
            vec![(Code::UnknownType, (2, 13)), (Code::Mismatch, (2, 19))],
        ),
        // A loop in a statement before the element's value keeps its own rules.
        (
            "fn main() {\n    let xs: Bad = [{\n        let y: Bad = loop {\n            if true { break 1 }\n            break true\n        }\n        1\n    }]\n}",
            vec![(Code::UnknownType, (2, 13)), (Code::UnknownType, (3, 16))],
        ),
    ] {
        codes(src, &expected);
    }
}

#[test]
fn a_part_that_fails_on_its_own_in_a_quiet_element_should_add_no_report() {
    // GIVEN a loop whose values do not join, in a part of the element that failed first
    let looped = "loop {\n        if true { break 1 }\n        break true\n    }";
    for (element, at, code) in [
        (format!("nope + {looped}"), (2, 20), Code::BindingNotFound),
        (format!("nope ?? {looped}"), (2, 20), Code::BindingNotFound),
        (format!("nope({looped})"), (2, 20), Code::FunctionNotFound),
        (
            format!("if true {{ nope }} else {{ {looped} }}"),
            (2, 30),
            Code::BindingNotFound,
        ),
    ] {
        // WHEN the element is checked in a literal whose context failed
        let src = format!("fn main() {{\n    let xs: Bad = [{element}]\n}}");
        // THEN only the failed context and the first failure are reported
        codes(&src, &[(Code::UnknownType, (2, 13)), (code, at)]);
    }
    codes(
        "fn f(b: u8) {\n    let xs: Bad = [nope + loop {\n        if true { break 1 }\n        break b\n    }]\n}\nfn main() {}",
        &[
            (Code::UnknownType, (2, 13)),
            (Code::BindingNotFound, (2, 20)),
        ],
    );
    // A loop that is the receiver of a field keeps its own report, as with no failed context.
    codes(
        &format!("fn main() {{\n    let xs: Bad = [{looped}.x]\n}}"),
        &[(Code::UnknownType, (2, 13)), (Code::Mismatch, (4, 15))],
    );
}

/// The branches are a nested join of the array: the array reports their conflict at its head
/// (ADR 0052).
#[test]
fn branches_that_do_not_join_in_a_quiet_element_should_still_be_reported() {
    for (src, at) in [
        (
            "fn main() {\n    let xs: Bad = [if true { [] } else { 1 }]\n}",
            (2, 19),
        ),
        (
            "fn main() {\n    let xs: Bad = [if true { 1 } else { [] }]\n}",
            (2, 19),
        ),
        (
            "fn main() {\n    let xs: Bad = [match true {\n        true => []\n        false => 1\n    }]\n}",
            (2, 19),
        ),
    ] {
        codes(src, &[(Code::UnknownType, (2, 13)), (Code::Mismatch, at)]);
    }
}

#[test]
fn empty_array_cannot_be_inferred() {
    diagnostic(
        "fn main() { let xs = [] }",
        Code::CannotInfer,
        (1, 22, "[]"),
        &[],
    );
}

#[test]
fn only_the_first_none_cannot_be_inferred() {
    codes(
        "fn main() { let x = [None, None] }",
        &[(Code::CannotInfer, (1, 22))],
    );
}

#[test]
fn empty_array_where_no_array_is_expected() {
    codes(
        "fn main() { let x: i64 = [] }",
        &[(Code::Mismatch, (1, 26))],
    );
}

/// The elements of an array that do not join give one report at its head (ADR 0052).
#[test]
fn elements_that_do_not_join_should_report_once_at_the_array() {
    codes(
        "fn main() { let a = [1, true] }",
        &[(Code::Mismatch, (1, 21))],
    );
    codes(
        "fn main() { print([1] == [true]) }",
        &[(Code::Mismatch, (1, 27))],
    );
}

#[test]
fn array_of_t_is_not_array_of_optional_t() {
    codes(
        &with_node("fn f(a: [Node]) { let b: [Node?] = a }"),
        &[(Code::Mismatch, (4, 36))],
    );
}

#[test]
fn fill_with_one_object() {
    diagnostic(
        &format!("{NODE}fn main() {{ let a = [Node(v: 1); 3] }}"),
        Code::SameObject,
        (4, 22, "Node(v: 1)"),
        &[],
    );
    diagnostic(
        &with_node("fn f(m: Node?) { let a = [m; 3] }"),
        Code::SameObject,
        (4, 27, "m"),
        &[],
    );
}

/// A fill reports an object under any layers of optional, of a class or a type parameter.
/// Every spelling of `None` holds no object, so it fills.
#[test]
fn fill_should_report_an_object_under_every_layer_of_optional() {
    for (src, expected) in [
        (
            "fn f(m: Node??) { let a = [m; 3] }",
            vec![(Code::SameObject, (4, 28))],
        ),
        (
            "fn f(m: Node???) { let a = [m; 3] }",
            vec![(Code::SameObject, (4, 29))],
        ),
        (
            "fn f<T>(m: T??) { let a = [m; 3] }",
            vec![(Code::SameObject, (4, 28))],
        ),
        ("fn f() { let a: [Node??] = [None; 3] }", vec![]),
        ("fn f() { let a: [Node?] = [Option.None; 3] }", vec![]),
        ("fn f() { let a = [Option<Node>.None; 3] }", vec![]),
        ("fn f<T>() { let a = [Option<T??>.None; 3] }", vec![]),
    ] {
        codes(&with_node(src), &expected);
    }
}

#[test]
fn fill_applies_the_class_rule_before_the_element_type() {
    codes(
        &with_node("fn f(n: Node) { let a: [i64] = [n; 3] }"),
        &[(Code::SameObject, (4, 33)), (Code::Mismatch, (4, 33))],
    );
}

#[test]
fn fill_length_is_i64() {
    codes(
        "fn main() { let a = [0; true] }",
        &[(Code::Mismatch, (1, 25))],
    );
}

#[test]
fn index_is_i64() {
    codes(
        "fn f(b: u8, xs: [i64]) { print(xs[b]) }\nfn main() {}",
        &[(Code::Mismatch, (1, 35))],
    );
}

#[test]
fn index_an_optional_array() {
    let d = diagnostic(
        "fn f(m: [i64]?) { print(m[0]) }\nfn main() {}",
        Code::MayBeNone,
        (1, 25, "m"),
        &[],
    );
    assert!(d.fixes().is_empty(), "{d:?}");
}

#[test]
fn index_a_non_array() {
    diagnostic(
        "fn f(n: i64) { print(n[0]) }\nfn main() {}",
        Code::CannotIndex,
        (1, 22, "n"),
        &[],
    );
    codes(
        &with_node("fn f(m: Node?) { print(m[0]) }"),
        &[(Code::CannotIndex, (4, 24))],
    );
}

#[test]
fn unknown_members_should_be_reported() {
    codes(
        "fn f(xs: [i64]) { xs.pop(1) }\nfn main() {}",
        &[(Code::NoMember, (1, 22))],
    );
    codes(
        "fn f(xs: [i64]) { print(xs.foo) }\nfn main() {}",
        &[(Code::NoMember, (1, 28))],
    );
}

#[test]
fn len_takes_no_arguments() {
    codes(
        "fn f(xs: [i64]) { print(xs.len(1)) }\nfn main() {}",
        &[(Code::ArgumentCount, (1, 25))],
    );
}

#[test]
fn len_is_not_a_field() {
    let src = "fn f(xs: [i64]) { print(xs.len) }\nfn main() {}";
    let d = diagnostic(src, Code::MethodNotField, (1, 28, "len"), &[]);
    assert_eq!(fixes(src, &d), [(true, vec![(1, 31, "", "()".to_owned())])]);
    fix_removes_its_code(src, &d);
}

#[test]
fn sum_and_contains_should_check_to_a_call_and_a_node() {
    with_prelude();
    contains(
        "fn f(xs: [i64]) -> i64 { xs.sum() }\nfn main() {}",
        &["(call [i64].sum xs#0)"],
    );
    contains(
        "fn f(xs: [u8?]) -> bool { xs.contains(5) }\nfn main() {}",
        &["(call [T].contains<u8?> xs#0 (wrap 5u8))"],
    );
    contains(
        "fn f(m: [i64]?) -> bool? { m?.contains(1) }\nfn main() {}",
        &["(call [T].contains<i64> #1 1)"],
    );
}

#[test]
fn contains_should_spill_its_base_for_an_exit_in_its_value() {
    with_prelude();
    contains(
        "fn f(xs: [[i64]], c: bool) -> bool { xs.contains(if c { return false } else { [1] }) }\nfn main() {}",
        &[
            "(block (let #2 xs#0) (let #3 (if c#1 (block (return false)) (block (array 1)))) (call [T].contains<[i64]> #2 #3))",
        ],
    );
}

#[test]
fn sum_of_an_array_that_is_not_numbers_should_need_numbers() {
    with_prelude();
    codes(
        "fn f(xs: [bool]) { print(xs.sum()) }\nfn main() {}",
        &[(Code::SumNeedsNumbers, (1, 29))],
    );
}

#[test]
fn contains_should_report_an_argument_that_is_not_an_element() {
    with_prelude();
    codes(
        "fn f(xs: [i64]) { print(xs.contains(true)) }\nfn main() {}",
        &[(Code::Mismatch, (1, 37))],
    );
}

/// A failed `contains` keeps its type `bool` when its argument is the problem, so a use of the
/// result is still checked.
#[test]
fn contains_should_keep_its_type_when_its_argument_fails() {
    with_prelude();
    codes(
        "fn f(xs: [i64]) {\n    let b = xs.contains(true)\n    print(b + 1)\n}\nfn main() {}",
        &[(Code::Mismatch, (2, 25)), (Code::Mismatch, (3, 11))],
    );
    codes(
        "fn f(xs: [i64]) {\n    let b = xs.contains(v: 1)\n    print(b + 1)\n}\nfn main() {}",
        &[(Code::LabeledArgument, (2, 25)), (Code::Mismatch, (3, 11))],
    );
}

/// An index that fails makes the receiver of `contains` untyped (the index row of S14). So the
/// call is untyped, and a use of the result reports nothing more.
#[test]
fn contains_should_be_untyped_when_its_receiver_fails() {
    with_prelude();
    codes(
        "fn f(g: [[i64]]) {\n    let b = g[nope].contains(1)\n    print(b + 1)\n}\nfn main() {}",
        &[(Code::BindingNotFound, (2, 15))],
    );
}

#[test]
fn a_wrong_count_of_method_arguments_should_be_the_general_count_error() {
    with_prelude();
    codes(
        "fn f(xs: [i64]) { print(xs.sum(1)) }\nfn main() {}",
        &[(Code::ArgumentCount, (1, 25))],
    );
    codes(
        "fn f(xs: [i64]) { print(xs.contains()) }\nfn main() {}",
        &[(Code::ArgumentCount, (1, 25))],
    );
    // A bound that fails does not change the count error of the call.
    codes(
        "class Node {}\nfn f(xs: [Node]) { print(xs.contains()) }\nfn main() {}",
        &[(Code::ArgumentCount, (2, 26))],
    );
}

#[test]
fn a_misread_comparison_should_explain_a_wrong_method_argument_count() {
    with_prelude();
    // For a method whose bound fails as for one whose bound holds.
    for (element, col) in [("i64", 53), ("Node", 54)] {
        codes(
            &format!(
                "class Node {{}}\nfn f(a: i64, b: i64, xs: [{element}]) {{ print(xs.contains(a < b, a > (b), 1)) }}\nfn main() {{}}"
            ),
            &[(Code::TypeArguments, (2, col))],
        );
    }
}

#[test]
fn len_of_an_optional_array() {
    let src = "fn f(m: [i64]?) { print(m.len()) }\nfn main() {}";
    let d = diagnostic(src, Code::MayBeNone, (1, 25, "m"), &[]);
    assert_eq!(
        fixes(src, &d),
        [(false, vec![(1, 26, ".", "?.".to_owned())])]
    );
    applied(src, &d);
}

#[test]
fn is_on_arrays() {
    diagnostic(
        "fn f(a: [i64], b: [i64]) { print(a is b) }\nfn main() {}",
        Code::IsNeedsObjects,
        (1, 34, "a"),
        &[],
    );
    diagnostic(
        "fn f(a: [i64]?, b: [i64]) { print(a is b) }\nfn main() {}",
        Code::IsNeedsObjects,
        (1, 35, "a"),
        &[],
    );
}

#[test]
fn print_should_accept_an_array_and_warn_on_an_optional_one() {
    clean("fn f(xs: [i64]) { print(xs) }\nfn main() {}");
    diagnostic(
        "fn f(m: [i64]?) { print(m) }\nfn main() {}",
        Code::PrintsOptional,
        (1, 25, "m"),
        &[],
    );
}

#[test]
fn unknown_element_type() {
    codes(
        "fn f(xs: [Nope]) {}\nfn main() {}",
        &[(Code::UnknownType, (1, 11))],
    );
}

#[test]
fn unknown_expected_type_is_reported_once() {
    for init in ["[]", "[None]", "[None; 2]"] {
        codes(
            &format!("fn main() {{ let a: Nope = {init} }}"),
            &[(Code::UnknownType, (1, 20))],
        );
    }
}

#[test]
fn unknown_name_poisons() {
    for (init, col) in [
        ("nope[0]", 13),
        ("[nope, 1]", 14),
        ("[nope, None]", 14),
        ("[nope; 3]", 14),
    ] {
        codes(
            &format!("fn main() {{\n    let x = {init}\n    print(x + true)\n}}"),
            &[(Code::BindingNotFound, (2, col))],
        );
    }
}

/// A `let` without a type binds no type when its value fails with no known type, so a use of the
/// name gives no error. A `let` with a type is in `check::tests`, in the test
/// `failed_value_of_an_annotated_let_should_keep_the_declared_type`.
#[test]
fn a_failed_value_of_an_unannotated_let_should_give_no_second_error() {
    for (src, code, pos) in [
        (
            "fn main() {\n    let x = [1, []]\n    print(x + true)\n}".to_owned(),
            Code::Mismatch,
            (2, 13),
        ),
        (
            format!("{NODE}fn main() {{\n    let x = [Node(v: 1); 3]\n    print(x + true)\n}}"),
            Code::SameObject,
            (5, 14),
        ),
        (
            "fn f(xs: [i64], b: u8) {\n    let x = xs[b]\n    print(x + true)\n}\nfn main() {}"
                .to_owned(),
            Code::Mismatch,
            (2, 16),
        ),
    ] {
        codes(&src, &[(code, pos)]);
    }
}

/// The one diagnostic of `src`, the idiom at `pos`: whether its one fix is safe, and `src`
/// with it applied. A safe fix leaves a program that checks clean.
fn idiom(src: &str, pos: (usize, usize, &str)) -> (bool, String) {
    let d = diagnostic(src, Code::MethodAsFunction, pos, &[]);
    let [fix] = d.fixes() else {
        panic!("one fix: {d:?}")
    };
    let safe = fix.safe();
    let result = applied(src, &d);
    if safe {
        clean(&result);
    }
    (safe, result)
}

#[test]
fn len_as_function_is_the_idiom() {
    assert_eq!(
        idiom(
            "fn f(xs: [i64]) -> i64 { len(xs) }\nfn main() {}",
            (1, 26, "len")
        ),
        (
            true,
            "fn f(xs: [i64]) -> i64 { xs.len() }\nfn main() {}".to_owned()
        )
    );
}

#[test]
fn contains_written_as_a_function_should_offer_the_method_call() {
    with_prelude();
    assert_eq!(
        idiom(
            "fn f(xs: [i64]) -> bool { contains(xs, 2) }\nfn main() {}",
            (1, 27, "contains")
        ),
        (
            true,
            "fn f(xs: [i64]) -> bool { xs.contains(2) }\nfn main() {}".to_owned()
        )
    );
}

#[test]
fn sum_as_function_on_u8_is_not() {
    codes(
        "fn f(bs: [u8]) -> i64 { sum(bs) }\nfn main() {}",
        &[(Code::FunctionNotFound, (1, 25))],
    );
}

#[test]
fn idiom_wraps_an_operator_receiver() {
    assert_eq!(
        idiom(
            "fn f(a: [i64]?, b: [i64]) -> i64 { len(a ?? b) }\nfn main() {}",
            (1, 36, "len")
        ),
        (
            true,
            "fn f(a: [i64]?, b: [i64]) -> i64 { (a ?? b).len() }\nfn main() {}".to_owned()
        )
    );
}

/// A parenthesised receiver keeps its parentheses, so the wrap may leave redundant ones; the
/// fix is still safe and the result checks clean.
#[test]
fn idiom_keeps_a_parenthesised_operator_receiver() {
    let (safe, _) = idiom(
        "fn f(m: [i64]?) -> i64 { len((m ?? [1])) }\nfn main() {}",
        (1, 26, "len"),
    );
    assert!(safe);
}

/// The receivers that take `.len()` as written; a parenthesised one keeps its parentheses.
#[test]
fn idiom_leaves_postfix_receivers_unwrapped() {
    let pre = "class Box {\n    xs: [i64]\n\n    fn all(self) -> [i64] {\n        self.xs\n    }\n}\nfn make() -> [i64] { [1] }\nfn f(b: Box, g: [[i64]], m: [i64]?) {\n    print(";
    for (arg, receiver) in [
        ("b.xs", "b.xs"),
        ("make()", "make()"),
        ("b.all()", "b.all()"),
        ("g[0]", "g[0]"),
        ("[1, 2]", "[1, 2]"),
        ("[0; 3]", "[0; 3]"),
        ("(g[0])", "(g[0])"),
    ] {
        let src = format!("{pre}len({arg}))\n}}\nfn main() {{}}");
        let (safe, result) = idiom(&src, (10, 11, "len"));
        assert!(safe, "{arg}");
        assert_eq!(
            result,
            format!("{pre}{receiver}.len())\n}}\nfn main() {{}}"),
            "{arg}"
        );
    }
}

/// An array whose own check failed is not known to be one, so the call falls back to "cannot
/// find function"; an argument whose check failed makes the fix unsafe.
#[test]
fn a_poisoned_operand_should_leave_the_idiom_unsafe_or_unfound() {
    with_prelude();
    codes(
        "fn f(x: i64) -> [i64] { [x] }\nfn main() {\n    print(len(f(nope)))\n}",
        &[
            (Code::FunctionNotFound, (3, 11)),
            (Code::BindingNotFound, (3, 17)),
        ],
    );
    let src = "fn id(x: i64) -> i64 { x }\nfn main() {\n    print(contains([1], id(nope)))\n}";
    codes(
        src,
        &[
            (Code::MethodAsFunction, (3, 11)),
            (Code::BindingNotFound, (3, 28)),
        ],
    );
    assert!(!diags(src)[0].fixes()[0].safe());
}

/// The rewrite keeps a comment between the name and the array, or after the array. A comment
/// before the array puts it in parentheses, so that the comment's line break stays inside them.
#[test]
fn the_idiom_should_keep_a_comment_and_be_safe() {
    with_prelude();
    for (src, pos, fixed) in [
        (
            "fn f(xs: [i64]) -> i64 {\n    len( // the list\n        xs)\n}\nfn main() {}",
            (2, 5, "len"),
            "fn f(xs: [i64]) -> i64 {\n    ( // the list\n        xs).len()\n}\nfn main() {}",
        ),
        (
            "fn f(xs: [i64]) -> bool {\n    contains(xs, // the value\n        2)\n}\nfn main() {}",
            (2, 5, "contains"),
            "fn f(xs: [i64]) -> bool {\n    xs.contains( // the value\n        2)\n}\nfn main() {}",
        ),
    ] {
        assert_eq!(idiom(src, pos), (true, fixed.to_owned()), "{src}");
    }
    // Without the comment, the same multi-line call stays safe.
    let src = "fn f(xs: [i64]) -> i64 {\n    len(\n        xs)\n}\nfn main() {}";
    assert!(idiom(src, (2, 5, "len")).0);
}

#[test]
fn the_idiom_should_be_unsafe_with_a_wrong_number_of_arguments() {
    with_prelude();
    assert_eq!(
        idiom(
            "fn f(xs: [i64]) -> i64 { len(xs, 1) }\nfn main() {}",
            (1, 26, "len")
        ),
        (
            false,
            "fn f(xs: [i64]) -> i64 { xs.len(1) }\nfn main() {}".to_owned()
        )
    );
    assert_eq!(
        idiom(
            "fn f(xs: [i64]) -> bool { contains(xs) }\nfn main() {}",
            (1, 27, "contains")
        ),
        (
            false,
            "fn f(xs: [i64]) -> bool { xs.contains() }\nfn main() {}".to_owned()
        )
    );
}

#[test]
fn the_idiom_should_be_unsafe_with_an_argument_of_the_wrong_type() {
    with_prelude();
    assert_eq!(
        idiom(
            "fn f(xs: [i64]) -> bool { contains(xs, true) }\nfn main() {}",
            (1, 27, "contains")
        ),
        (
            false,
            "fn f(xs: [i64]) -> bool { xs.contains(true) }\nfn main() {}".to_owned()
        )
    );
}

#[test]
fn the_idiom_should_be_safe_with_a_literal_that_fits_a_u8_array() {
    with_prelude();
    assert_eq!(
        idiom(
            "fn f(bs: [u8]) -> bool { contains(bs, 1) }\nfn main() {}",
            (1, 26, "contains")
        ),
        (
            true,
            "fn f(bs: [u8]) -> bool { bs.contains(1) }\nfn main() {}".to_owned()
        )
    );
}

#[test]
fn the_idiom_should_be_safe_for_a_u8_argument_and_unsafe_for_an_i64_one() {
    with_prelude();
    for (param, col, safe) in [("n: i64", 34, false), ("n: u8", 33, true)] {
        let src = format!("fn f(bs: [u8], {param}) -> bool {{ contains(bs, n) }}\nfn main() {{}}");
        assert_eq!(idiom(&src, (1, col, "contains")).0, safe, "{param}");
    }
}

#[test]
fn the_idiom_should_be_unsafe_with_a_literal_out_of_u8_range() {
    with_prelude();
    // The argument is checked against the method's parameter, as for any method of the prelude,
    // so the literal that does not fit is reported beside the idiom.
    let src = "fn f(bs: [u8]) -> bool { contains(bs, 256) }\nfn main() {}";
    codes(
        src,
        &[
            (Code::MethodAsFunction, (1, 26)),
            (Code::IntegerDoesNotFit, (1, 39)),
        ],
    );
    assert!(!diags(src)[0].fixes()[0].safe());
}

#[test]
fn sum_written_as_a_function_should_offer_the_method_call() {
    with_prelude();
    assert_eq!(
        idiom(
            "fn f(xs: [i64]) -> i64 { sum(xs) }\nfn main() {}",
            (1, 26, "sum")
        ),
        (
            true,
            "fn f(xs: [i64]) -> i64 { xs.sum() }\nfn main() {}".to_owned()
        )
    );
}

#[test]
fn no_argument_is_not_the_idiom() {
    codes("fn main() { len() }", &[(Code::FunctionNotFound, (1, 13))]);
}

#[test]
fn non_array_argument_is_not_found() {
    codes(
        "fn f(n: i64) -> i64 { len(n) }\nfn main() {}",
        &[(Code::FunctionNotFound, (1, 23))],
    );
}

#[test]
fn poisoned_argument_is_not_found() {
    codes(
        "fn main() {\n    print(len(nope))\n}",
        &[
            (Code::FunctionNotFound, (2, 11)),
            (Code::BindingNotFound, (2, 15)),
        ],
    );
}

#[test]
fn declared_len_is_called() {
    clean("fn len(xs: [i64]) -> i64 { 0 }\nfn f(xs: [i64]) -> i64 { len(xs) }\nfn main() {}");
}

#[test]
fn labeled_argument_is_not_the_idiom() {
    codes(
        "fn f(a: [i64]) -> i64 { len(xs: a) }\nfn main() {}",
        &[(Code::FunctionNotFound, (1, 25))],
    );
}

/// Every argument of an idiom call, or of the call it falls back to, is checked exactly once: the
/// complete ordered list of diagnostics.
#[test]
fn each_argument_of_the_idiom_should_be_checked_once() {
    with_prelude();
    let class_len =
        "class len {\n    xs: [i64]\n}\nfn f(xs: [i64]) {\n    len(xs)\n}\nfn main() {}";
    for (src, expected) in [
        (
            "fn f(g: [[i64]]) -> i64 { len(g[nope], bad) }\nfn main() {}",
            vec![
                (Code::FunctionNotFound, (1, 27)),
                (Code::BindingNotFound, (1, 33)),
                (Code::BindingNotFound, (1, 40)),
            ],
        ),
        (
            "fn f(xs: [i64]) -> bool { contains(xs, missing, other) }\nfn main() {}",
            vec![
                (Code::MethodAsFunction, (1, 27)),
                (Code::BindingNotFound, (1, 40)),
                (Code::BindingNotFound, (1, 49)),
            ],
        ),
        (
            "fn f(n: i64) -> i64 { len(n, missing) }\nfn main() {}",
            vec![
                (Code::FunctionNotFound, (1, 23)),
                (Code::BindingNotFound, (1, 30)),
            ],
        ),
        (
            "fn f() -> i64 { len(nope, missing) }\nfn main() {}",
            vec![
                (Code::FunctionNotFound, (1, 17)),
                (Code::BindingNotFound, (1, 21)),
                (Code::BindingNotFound, (1, 27)),
            ],
        ),
        (
            "fn f(xs: [i64]) -> bool { contains(xs, v: missing) }\nfn main() {}",
            vec![
                (Code::FunctionNotFound, (1, 27)),
                (Code::BindingNotFound, (1, 43)),
            ],
        ),
        (class_len, vec![(Code::ArgumentNeedsLabel, (5, 9))]),
    ] {
        codes(src, &expected);
    }
}

/// A `push` checks its argument as an assignment's value.
#[test]
fn push_should_check_its_argument_against_the_element_type() {
    codes(
        "fn main() {\n    let mut xs = [1]\n    xs.push(\"a\")\n}",
        &[(Code::Mismatch, (3, 13))],
    );
}

/// A `push` takes exactly one argument.
#[test]
fn push_should_take_exactly_one_argument() {
    for (row, src) in [
        (
            "no argument",
            "fn main() {\n    let mut xs = [1]\n    xs.push()\n}",
        ),
        (
            "two arguments",
            "fn main() {\n    let mut xs = [1]\n    xs.push(1, 2)\n}",
        ),
    ] {
        codes_row(row, src, &[(Code::ArgumentCount, (3, 5))]);
    }
}

/// The declarations of the join tests: `id`, `none`, `pick` and `k2` are calls that only a sibling
/// can type (`k2` returns its second argument), `f` and `g` take an array that holds a type
/// parameter.
const JOIN_DECLS: &str = "fn id<T>(x: T) -> T { x }\nfn none<T>() -> T? { None }\n\
    fn pick<T>(a: T, b: T) -> T { a }\nfn k2<T, U>(a: T, b: U) -> U { b }\n\
    fn f<T>(x: [T]) {}\nfn g<T>(x: [[T?]]) {}\n";

/// `statement` at line 6 of a `main` with `c: bool`, `x: u8`, `x2: [u8]` and `y: u8?`, after the
/// `JOIN_DECLS`.
fn join_main(statement: &str) -> String {
    format!(
        "{JOIN_DECLS}fn main() {{\n    let c = true\n    let x: u8 = 3\n    let x2: [u8] = [1]\n    let y: u8? = 4\n    {statement}\n}}"
    )
}

/// The place of `needle` in `statement`, which `join_main` put in `src`.
fn in_statement(src: &str, statement: &str, needle: &str) -> (usize, usize) {
    let start = src
        .rfind(statement)
        .expect("the program should hold the statement");
    let inside = statement
        .find(needle)
        .expect("the statement should hold the text");
    line_col(src, start + inside)
}

/// Siblings in an array literal rank by what they hold, as the arguments of a call do. A value
/// that waits for a type, and a block, a call or an `if` that holds a literal, take the type of the
/// others. Before, a block or a generic call counted as concrete, was checked alone and gave the
/// default type or no type (#287, #289), and `dbg(None)` made the type of the literal optional for
/// its context but not for its elements (a crash in the code generator).
#[test]
fn siblings_should_take_the_type_of_the_others_in_an_array_literal() {
    with_prelude();
    for statement in [
        // `dbg` passes the context on: the element type is optional.
        "let _a = [dbg(None), 1]",
        "let _a = [1, dbg(None)]",
        "let _a = [{ None }, 1]",
        "let _a = [id([]), [1.0]]",
        "let _a = [[1], id([])]",
        "let _a = [[[], []], [[1]]]",
        // A literal under a block or a call takes the `u8` of its sibling, at each place.
        "let _a = [{ 1 }, x]",
        "let _a = [id(1), x]",
        "let _a = [{ 1 }, x, 2]",
        "let _a = [x, { 1 }, 2]",
        "let _a = [x, 2, { 1 }]",
        "let _a = [x, if c { 1 } else { None }]",
        // A call that makes an optional, by its arguments or by its declaration, makes the
        // element type optional, as the `None` or the `if` it holds does.
        "let _a = [id(None), 1]",
        "let _a = [1, none()]",
        "let _a = [none(), x]",
        "let _a = [x, id(None)]",
        "let _a = [x, none()]",
        "let _a = [x, dbg(id(None))]",
        "let _a = [x, pick(None, None)]",
        "let _a = [id(if c { 1 } else { None })]",
        "let _a = [1, id(if c { 1 } else { None })]",
        "let _a = [x, if c { 1 } else { 2 }]",
        "let _a = [x, match c {\n        true => 1\n        false => None\n    }]",
        "g([[None], [Some(\"s\")]])",
        // Two arrays deep.
        "let _a = [[x, { 1 }], x2]",
    ] {
        clean_row(statement, &join_main(statement));
    }
}

/// A value that waits is typed as its siblings are, not as an optional of it, unless it makes an
/// optional by itself. So a `[]` is reported for what it is.
#[test]
fn a_waiting_sibling_that_is_no_optional_should_be_reported_for_its_type() {
    with_prelude();
    // The report of the join is at its head, and an array in a call is a nested join of the
    // call (ADR 0052).
    for (statement, head) in [
        ("let _a = [[], 1]", "["),
        ("f([[], 1])", "f"),
        ("let _a = [1, []]", "["),
    ] {
        let src = join_main(statement);
        codes_row(
            statement,
            &src,
            &[(Code::Mismatch, in_statement(&src, statement, head))],
        );
    }
}

/// A mistake that remains is reported where it was before.
#[test]
fn a_sibling_with_nothing_to_wait_for_should_be_reported_once() {
    for (statement, expected) in [
        ("let _a = [None]", (Code::CannotInfer, "None")),
        ("let _a = [[], []]", (Code::CannotInfer, "[]")),
    ] {
        let src = join_main(statement);
        let (code, text) = expected;
        codes_row(
            statement,
            &src,
            &[(code, in_statement(&src, statement, text))],
        );
    }
}

/// A literal at a place that the call does not return does not give the call its type (see
/// `Checker::rank_call`, `passes`). `k2(1, [])` returns its second argument, as `id([])` does: the
/// `1` is not walked, and the call ranks as `id([])` ranks. So `[[x], k2(1, [])]` checks clean, and
/// its `[]` takes the type of `[x]`. If the `1` made the call a literal, the call would be checked
/// alone and `U` could not be inferred.
#[test]
fn a_literal_at_a_place_that_is_not_returned_should_not_type_the_call() {
    let statement = "let _a = [[x], k2(1, [])]";
    clean_row(statement, &join_main(statement));
}

/// `dbg` with two arguments is not the one-argument `dbg` that passes its context on: in an
/// array that a call takes, it reports its argument count once, as `dbg_argument` decides.
#[test]
fn a_dbg_with_two_arguments_in_an_array_should_report_the_count_once() {
    with_prelude();
    let statement = "g([[dbg(1, 2)]])";
    let src = join_main(statement);
    codes(
        &src,
        &[(Code::PrintArgument, in_statement(&src, statement, "dbg"))],
    );
}

/// A literal that holds an array takes its type from a sibling, as a scalar literal does: the
/// elements of an array are members of one join (ADR 0052), so `[[1], x2]` is `[[u8]]`.
#[test]
fn a_literal_array_should_take_its_type_beside_a_sibling() {
    for statement in [
        "let _a = [[1], [2]]",
        "let _a: [i64?] = [Option<i64>.None, 1]",
        "let _a = if c { [1] } else { [] }",
        "let _a = [[1], x2]",
        "let _a = [x2, [1]]",
    ] {
        clean_row(statement, &join_main(statement));
    }
}

/// A block, a call or an `if` that ranks a sibling is checked once. The statements of a block
/// that ranking checks ahead report their mistakes once.
#[test]
fn work_done_to_rank_a_sibling_should_report_its_mistakes_once() {
    for statement in [
        "let _a = [{ let z = undefined_name; 1 }, x]",
        "let _a = [x, { let z = undefined_name; 1 }]",
        "let _a = if c { let z = undefined_name; 1 } else { x }",
        "let _a = if c { x } else { let z = undefined_name; 1 }",
        "let _a = { let z = undefined_name; 1 } + x",
        "let _a = ({ let z = undefined_name; 1 }) == x",
        "let _a = x < { let z = undefined_name; 1 }",
        "let _a = [undefined_name.len(), 2]",
        "let _a = undefined_name.len() + x",
        "let _a = match c {\n        true => { let z = undefined_name; 1 }\n        false => x\n    }",
    ] {
        let src = join_main(statement);
        codes_row(
            statement,
            &src,
            &[(
                Code::BindingNotFound,
                in_statement(&src, statement, "undefined_name"),
            )],
        );
    }
}

/// A `push` writes the place of its receiver, so ranking does not check the receiver ahead as a
/// value. A branch that ends in a `push`, inside a block that ranking checks ahead as a sibling,
/// checks clean. Before, the call got no place and failed with no diagnostic.
#[test]
fn a_push_that_ends_a_branch_should_check_clean_when_a_sibling_ranks_it() {
    for statement in [
        "let _a = [[1], ({\n        if c {\n        } else {\n            let mut v: [string?] = []\n            v.push(None)\n        }\n        [2]\n    })][0]",
        "let _a = { if c {\n        } else {\n            let mut v: [i64] = []\n            v.push(1)\n        }\n        2 } + x",
    ] {
        clean_row(statement, &join_main(statement));
    }
}

/// A call of a requirement of a bound, with no argument to give its `U`, waits for a type as a
/// call of a function does. Beside an `i64` it takes that type, its `U?` made optional, so the
/// array is `[i64?]`. A call that did not wait would be checked alone, and `U` could not be
/// inferred.
#[test]
fn a_requirement_call_that_waits_should_take_the_type_of_its_sibling() {
    let src = "interface G {\n    fn ropt<U>(self) -> U?\n}\n\
        fn f<X: G>(x: X) {\n    let a = [x.ropt(), 1]\n}\nfn main() {}";
    clean(src);
}

/// GIVEN an array whose elements join an `any` value with an optional, `None` or a `Some` of an
///   optional
/// AND the `if` that joins the same values as its branches
/// WHEN the program is checked
/// THEN the array is valid, as its twin is: its elements are members of one join (spec, S1)
/// AND an array in a branch is a nested join, whose elements are members of the `if`'s join
#[test]
fn an_array_should_accept_what_its_if_twin_accepts() {
    let rows = [
        (
            "let b = [Some(2), d]",
            "let b = if c { Some(2) } else { d }",
        ),
        ("let f = [d, opt(2)]", "let f = if c { d } else { opt(2) }"),
        (
            "let e = [o, Some(Some(2))]",
            "let e = if c { o } else { Some(Some(2)) }",
        ),
        (
            "let g = if c { [d] } else { [None] }",
            "let g = if c { d } else { None }",
        ),
        (
            "let g = if c { [d] } else { [1, None] }",
            "let g = if c { d } else if c { 1 } else { None }",
        ),
        (
            "let b = if c { [o] } else { [Some(Some(2))] }",
            "let b = if c { o } else { Some(Some(2)) }",
        ),
    ];
    for (array, twin) in rows {
        for row in [array, twin] {
            let src = format!(
                "fn opt<T>(x: T) -> T? {{ Some(x) }}\nfn main() {{\n    let c = [1].len() == 2\n    \
                 let d: any Display = 1\n    let o: (any Display)? = Some(7)\n    {row}\n}}"
            );
            clean_row(row, &src);
        }
    }
}

/// GIVEN a call whose array argument gives the parameter `U` one type and `V` two types
/// AND the elements are at each depth, and beside `None`
/// WHEN the program is checked
/// THEN the call reports the conflict of `V` once, at its head (ADR 0052)
#[test]
fn a_conflict_in_array_elements_should_report_once_at_the_call() {
    let decls = "enum Two<A, B> { P(A, B) }\nfn hh<U, V>(a: U, b: [[Two<U, V>?]]) {}\n\
                 fn h1<U, V>(a: U, b: [Two<U, V>?]) {}\nfn h<U, V>(a: U, b: [Two<U, V>]) {}\n";
    for row in [
        "hh(true, [[Some(Two.P(true, 1)), None], [Some(Two.P(false, \"x\"))]])",
        "h1(true, [Some(Two.P(true, 1)), None, Some(Two.P(false, \"x\"))])",
        "hh(true, [[Some(Two.P(true, 1))], [Some(Two.P(false, \"x\"))]])",
        "hh(true, [[None, Some(Two.P(true, 1))], [Some(Two.P(false, \"x\"))]])",
        "h(true, [Two.P(true, 1), Two.P(true, \"s\")])",
        "h(true, [Two.P(true, \"s\"), Two.P(false, 1)])",
    ] {
        let src = format!("{decls}fn main() {{\n    {row}\n}}");
        codes(&src, &[(Code::Mismatch, (6, 5))]);
    }
}

/// The source of `main`, whose `let` takes `n` layers of `open`, `1` and `close` around `1`.
fn nested_layers(n: usize, (open, close): (&str, &str)) -> String {
    format!(
        "fn main() {{\n    let a = {}1{}\n    print(\"{{a.len()}}\")\n}}\n",
        open.repeat(n),
        close.repeat(n)
    )
}

/// The expressions that `gives_made_value` and `without_value` visit when `main` is checked,
/// whose `let` takes `n` nested layers of `layer` around `1`.
fn nested_array_walk_steps(n: usize, layer: (&str, &str)) -> usize {
    let src = nested_layers(n, layer);
    VALUE_WALK_STEPS.set(0);
    let (_, ds) = checked_with(PRELUDE_SOURCE, &src);
    assert!(errors(&src, &ds).is_empty(), "{n} layers of {layer:?}");
    VALUE_WALK_STEPS.get()
}

#[test]
fn nested_array_literal_should_cost_linear_value_walk_steps() {
    // GIVEN `let a = [[...[1]...]]` and `let a = [[...[1; 2]...; 2]; 2]` of 48, 96 and 144 layers
    for (row, layer) in [("literal", ("[", "]")), ("fill", ("[", "; 2]"))] {
        // WHEN each is checked
        let steps = [48, 96, 144].map(|n| nested_array_walk_steps(n, layer));
        // THEN no layer walks the layers inside it again, so the steps grow linearly
        assert_near_linear_for(&format!("row `{row}`: "), steps[0], steps[1], steps[2]);
    }
}

/// The value positions that `converts_at` visits when `main` is checked, whose `let` takes `n`
/// nested layers around `[1]`, each beside a `[]`.
fn beside_empty_converts_steps(n: usize) -> usize {
    let mut value = "[1]".to_string();
    for _ in 0..n {
        value = format!("[{value}, []]");
    }
    let src = format!("fn main() {{\n    let a = {value}\n    print(\"{{a.len()}}\")\n}}\n");
    join::CONVERTS_STEPS.set(0);
    let (_, ds) = checked_with(PRELUDE_SOURCE, &src);
    assert!(errors(&src, &ds).is_empty(), "{n} layers");
    join::CONVERTS_STEPS.get()
}

#[test]
fn nested_array_beside_an_empty_array_should_cost_linear_converts_steps() {
    // GIVEN `let a = [[[[1], []], []], ...]` of 40, 80 and 120 layers
    // WHEN each is checked
    let steps = [40, 80, 120].map(beside_empty_converts_steps);
    // THEN the join tests one candidate, not one for each `[]`, so the steps grow linearly
    assert_near_linear(steps[0], steps[1], steps[2]);
}

/// GIVEN a generic call whose argument holds a nested leaf that fits no type of the parameter
/// AND the leaf is `0` in `if c { row } else { 0 }` at `[T]`, or `1` in `[[None], 1]` at `[[T?]]`
/// WHEN the program is checked
/// THEN the call gives one E0032 at its head, with a label at the leaves, and no E0050
#[test]
fn a_nested_leaf_that_fits_no_type_of_the_parameter_should_give_e0032_at_the_call() {
    let decls = "struct P<A> {\n    a: A\n}\n\
                 fn flat<T>(xs: [[T]]) -> i64 { xs.len() }\n\
                 fn one<T>(xs: [T]) -> i64 { xs.len() }\n\
                 fn oner<T>(xs: [T]) -> [T] { xs }\n\
                 fn two<T>(xs: [T], y: T) -> i64 { xs.len() }\n\
                 fn opt<T>(xs: [T?]) -> i64 { xs.len() }\n\
                 fn bx<T>(xs: [P<T>]) -> i64 { xs.len() }\n\
                 fn g4<T>(xs: [[T?]]) -> i64 { xs.len() }\n\
                 fn main() {\n    let row = [1, 2]\n    let o: i64? = None\n\
                 \x20   let c = [1].len() == 2\n    let d = [1].len() == 3\n";
    // Each row, with the columns of the labels on its line.
    let rows: [(&str, &[usize]); 25] = [
        ("one(if c { row } else { 0 })", &[22]),
        ("flat([row, 3])", &[17, 22]),
        ("one(if c { [1] } else { None })", &[23, 35]),
        ("flat([[1, 2], [3], 4])", &[18, 21, 26, 30]),
        ("flat([[1], \"s\"])", &[18, 22]),
        ("one(if c { [1] } else { 2 })", &[23, 35]),
        ("one(if c { row } else if c { [1] } else { 2 })", &[22]),
        ("flat([row, row, None])", &[17, 22, 27]),
        ("flat([row, Some(row)])", &[17, 27]),
        ("one(if c { row } else { None })", &[22, 35]),
        ("one({ if c { row } else { 0 } })", &[24]),
        ("flat([if c { row } else { 0 }, row])", &[24, 37, 42]),
        ("flat([row, Some(3)])", &[17, 27]),
        ("oner(if c { row } else { 0 }).len()", &[23]),
        ("one(dbg(if c { row } else { 0 }))", &[26]),
        ("flat([dbg(row), 0])", &[21, 27]),
        ("two(if c { row } else { 0 }, 1)", &[22]),
        ("opt(if c { [o] } else { o })", &[23, 35]),
        ("bx([P(a: 1), 2])", &[15, 24]),
        ("bx(if c { [P(a: 1)] } else { P(a: 2) })", &[22, 40]),
        ("g4([[None], 1])", &[16, 23]),
        ("g4([1, [None]])", &[15, 19]),
        ("g4([[None], [1], 1])", &[16, 24, 28]),
        (
            "g4([if c { [None] } else if d { [1] } else { 1 }])",
            &[23, 44, 56],
        ),
        (
            "opt(if c { [None] } else if d { [1] } else { 1 })",
            &[23, 44, 56],
        ),
    ];
    let mut red = Vec::new();
    for (row, labels) in rows {
        let src = format!("{decls}    print({row})\n}}\n");
        let wanted = vec![(
            Code::Mismatch,
            (16, 11),
            labels.iter().map(|&col| (16, col)).collect::<Vec<_>>(),
        )];
        let found = found(&src);
        if found != wanted {
            red.push(format!("row `{row}`: found {found:?}"));
        }
    }
    assert!(
        red.is_empty(),
        "{} red rows:\n{}",
        red.len(),
        red.join("\n")
    );
}

/// GIVEN an array literal and an `if` whose values differ, each in parentheses with a comment
/// before its head
/// WHEN the program is checked
/// THEN the E0032 of each is at its `[` or its `if`, not at the comment
#[test]
fn the_head_of_a_join_in_parentheses_should_be_its_bracket_or_keyword() {
    let src = "fn main() {\n    let c = [1].len() == 2\n    let x = (\n        // the rows\n        \
               [1, \"s\"])\n    let y = (\n        // the branches\n        \
               (if c { 1 } else { \"s\" }))\n    print(x.len())\n}\n";
    let heads: Vec<_> = found(src)
        .iter()
        .map(|(code, at, _)| (*code, *at))
        .collect();
    assert_eq!(heads, [(Code::Mismatch, (5, 9)), (Code::Mismatch, (8, 10))]);
}

/// Each order of an array of `depth` layers with a `None` beside the array in each layer. The
/// innermost layer holds `1` and `None`: for two layers, `[[1, None], None]`, `[None, [None, 1]]`
/// and the two others.
fn none_beside_each_layer(depth: usize) -> Vec<String> {
    let mut rows = vec!["1".to_string()];
    for _ in 0..depth {
        rows = (rows.iter())
            .flat_map(|row| [format!("[{row}, None]"), format!("[None, {row}]")])
            .collect();
    }
    rows
}

/// GIVEN an array of three layers or of four, with a `None` beside the array in each layer
/// AND each order of the members of each layer, as `[[[1, None], None], None]` and
///   `[[[None, 1], None], None]`
/// WHEN each is the value of a `let` with no type
/// THEN each has the type that `main` gives it, `[[[i64?]?]?]` or `[[[[i64?]?]?]?]`, in each order
#[test]
fn a_none_beside_each_layer_of_a_nested_array_should_give_one_type_in_each_order() {
    let mut red = Vec::new();
    for (depth, ty) in [(3, "[[[i64?]?]?]"), (4, "[[[[i64?]?]?]?]")] {
        for row in none_beside_each_layer(depth) {
            let src = format!(
                "fn main() {{\n    let a = {row}\n    let t: {ty} = a\n    print(t.len())\n}}\n"
            );
            let found = found(&src);
            if !found.is_empty() {
                red.push(format!("row `{row}`: found {found:?}"));
            }
        }
    }
    assert!(
        red.is_empty(),
        "{} red rows:\n{}",
        red.len(),
        red.join("\n")
    );
}

/// GIVEN a nested array of a generic struct or enum leaf, a `Some` of the leaf beside it, and a
///   `None` one layer up.
/// AND each order of the first layer. An example is `[[[P(a: 1), Some(P(a: 1))]], [[P(a: 1)],
///   None]]`, and its other order is `[[[Some(P(a: 1)), P(a: 1)]], [[P(a: 1)], None]]`
/// WHEN each is the value of a `let` with no type
/// THEN each has one type in each order, `[[[P<i64>?]?]]` or `[[[E<i64>?]?]]`
#[test]
fn a_generic_leaf_beside_each_layer_of_a_nested_array_should_give_one_type_in_each_order() {
    let rows = [
        (
            "[[[P(a: 1), Some(P(a: 1))]], [[P(a: 1)], None]]",
            "[[[P<i64>?]?]]",
        ),
        (
            "[[[Some(P(a: 1)), P(a: 1)]], [[P(a: 1)], None]]",
            "[[[P<i64>?]?]]",
        ),
        ("[[[E.X(1), None]], [[E.X(1)], None]]", "[[[E<i64>?]?]]"),
        ("[[[None, E.X(1)]], [[E.X(1)], None]]", "[[[E<i64>?]?]]"),
        ("[[[E.Y, None]], [[E.X(1)], None]]", "[[[E<i64>?]?]]"),
        ("[[[None, E.Y]], [[E.X(1)], None]]", "[[[E<i64>?]?]]"),
    ];
    let mut red = Vec::new();
    for (row, ty) in rows {
        let src = format!(
            "struct P<A> {{\n    a: A\n}}\nenum E<A> {{\n    X(A)\n    Y\n}}\nfn main() {{\n    let a = {row}\n    let t: {ty} = a\n    print(t.len())\n}}\n"
        );
        let found = found(&src);
        if !found.is_empty() {
            red.push(format!("row `{row}`: found {found:?}"));
        }
    }
    assert!(
        red.is_empty(),
        "{} red rows:\n{}",
        red.len(),
        red.join("\n")
    );
}
