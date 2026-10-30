//! The prelude's language interfaces (`Display`, `Eq`, `Ord`, `AnyObject`) and the conformances
//! that the compiler writes for the types: the rules in `Checker::conforms`.

use std::fmt::Write as _;

use super::*;
use crate::check::cycle;
use crate::check::interface::{CONFORMS_DEPTH_LIMIT, LangPath};

const _: () = assert!(
    CONFORMS_DEPTH_LIMIT == 1 << 18,
    "a conformance query should stop when more than 2^18 class or enum keys are open"
);

/// The functions that ask for each language interface, with bodies that call nothing.
const NEEDS: &str = "fn need_eq<T: Eq>(x: T) {}\nfn need_ord<T: Ord>(x: T) {}\nfn need_display<T: Display>(x: T) {}\nfn need_object<T: AnyObject>(x: T) {}\n";

/// A program that asks `need_<interface>` of a parameter of type `ty`, after `decls`.
fn asking(decls: &str, interface: &str, ty: &str) -> String {
    format!("{NEEDS}{decls}\nfn f(x: {ty}) {{\n    need_{interface}(x)\n}}\nfn main() {{}}")
}

/// Where the argument of the call `call` (`need_eq(x)`) is in `src`.
fn argument(src: &str, call: &str) -> (usize, usize) {
    let start = src.find(call).expect("the call should be in the program");
    line_col(
        src,
        start + call.find('(').expect("a call has an argument list") + 1,
    )
}

/// `src` with `calls` as the body of `fn f`, after `decls`, where each call names a parameter.
fn calling(decls: &str, params: &str, calls: &[&str]) -> String {
    let body: Vec<String> = calls.iter().map(|call| format!("    {call}\n")).collect();
    format!(
        "{NEEDS}{decls}\nfn f({params}) {{\n{}}}\nfn main() {{}}",
        body.concat()
    )
}

/// The `NotConform` that each call in `failing` gets, in the order of the calls in `src`.
fn failures(src: &str, failing: &[&str]) -> Vec<(Code, (usize, usize))> {
    failing
        .iter()
        .map(|call| (Code::NotConform, argument(src, call)))
        .collect()
}

const NODE: &str = "class Node {}";
const DIR: &str = "enum Dir {\n    North\n    South\n}";
const EXPR: &str = "enum Expr {\n    Num(i64)\n    Add(Expr, Expr)\n}";
const HOLDS_NODE: &str = "class Node {}\nenum H {\n    N(Node)\n}";
const GROWS_NODE: &str = "class Node {}\nenum W<T> {\n    A(T)\n    B(W<Node>)\n}";
const PQ: &str =
    "class Node {}\nenum P<T> {\n    A(Q<T>)\n}\nenum Q<T> {\n    B(P<T>)\n    C(T)\n}";

#[test]
fn generated_conformances_should_follow_the_spec_table() {
    with_prelude();
    // Each row: the declarations, the parameter type, and whether `Eq`, `Ord` and `Display` hold.
    let rows: [(&str, &str, &str, [bool; 3]); 25] = [
        ("i64", "", "i64", [true, true, true]),
        ("u8", "", "u8", [true, true, true]),
        ("char", "", "char", [true, true, true]),
        ("string", "", "string", [true, true, true]),
        ("f64", "", "f64", [true, true, true]),
        ("bool has no Ord", "", "bool", [true, false, true]),
        ("array", "", "[i64]", [true, false, true]),
        ("nested array", "", "[[i64]]", [true, false, true]),
        ("optional", "", "i64?", [true, false, true]),
        ("array of optionals", "", "[string?]", [true, false, true]),
        ("array of classes", NODE, "[Node]", [false, false, true]),
        ("optional class", NODE, "Node?", [false, false, true]),
        ("class", NODE, "Node", [false, false, true]),
        ("enum of scalars", DIR, "Dir", [true, false, true]),
        ("recursive enum", EXPR, "Expr", [true, false, true]),
        (
            "enum holding a class",
            HOLDS_NODE,
            "H",
            [false, false, true],
        ),
        ("array of enums", DIR, "[Dir]", [true, false, true]),
        (
            "array of recursive enums",
            EXPR,
            "[Expr?]",
            [true, false, true],
        ),
        (
            "enum that meets a class",
            GROWS_NODE,
            "W<i64>",
            [false, false, true],
        ),
        (
            "enums that need each other, with Eq",
            PQ,
            "P<i64>",
            [true, false, true],
        ),
        (
            "enums that need each other, without Eq",
            PQ,
            "P<Node>",
            [false, false, true],
        ),
        ("result", "", "Result<i64, string>", [true, false, true]),
        (
            "result with a class",
            NODE,
            "Result<i64, Node>",
            [false, false, true],
        ),
        (
            "array of optional arrays",
            "",
            "[[i64]?]",
            [true, false, true],
        ),
        ("unit-like enum array", DIR, "[[Dir]]", [true, false, true]),
    ];
    for (name, decls, ty, [eq, ord, display]) in rows {
        for (interface, passes) in [("eq", eq), ("ord", ord), ("display", display)] {
            let src = asking(decls, interface, ty);
            let row = format!("{name}: {interface}");
            if passes {
                clean_row(&row, &src);
            } else {
                let at = argument(&src, &format!("need_{interface}(x)"));
                codes_row(&row, &src, &[(Code::NotConform, at)]);
            }
        }
    }
}

#[test]
fn generated_any_object_should_hold_for_classes_only() {
    for (name, decls, ty, passes) in [
        ("class", "class Node {}", "Node", true),
        (
            "generic class",
            "class Box<T> {\n    v: T\n}",
            "Box<i64>",
            true,
        ),
        ("number", "", "i64", false),
        ("enum", "enum Dir {\n    North\n}", "Dir", false),
        ("array of classes", "class Node {}", "[Node]", false),
        ("optional class", "class Node {}", "Node?", false),
        ("string", "", "string", false),
    ] {
        let src = asking(decls, "object", ty);
        if passes {
            clean_row(name, &src);
        } else {
            let at = argument(&src, "need_object(x)");
            codes_row(name, &src, &[(Code::NotConform, at)]);
        }
    }
}

#[test]
fn generated_eq_should_follow_the_bounds_of_a_type_parameter() {
    let wrap = "enum Wrap<T> {\n    A(T)\n}";
    let calls = ["need_eq(a)", "need_eq(b)", "need_eq(c)"];
    for (row, bound, passes) in [("bounded", "T: Eq", true), ("unbounded", "T", false)] {
        let src = calling(wrap, "", &calls).replace(
            "fn f()",
            &format!("fn f<{bound}>(a: [T], b: T?, c: Wrap<T>)"),
        );
        if passes {
            clean_row(row, &src);
        } else {
            codes_row(row, &src, &failures(&src, &calls));
        }
    }
    // `Display` holds for a type parameter whatever its bounds.
    let shown = calling("", "", &["need_display(x)"]).replace("fn f()", "fn f<T>(x: T)");
    clean_row("display of a parameter", &shown);
}

#[test]
fn generated_eq_of_a_type_parameter_should_not_be_kept_for_the_next_body() {
    let wrap = "enum Wrap<T> {\n    A(T)\n}\n";
    let yes = "fn yes<T: Eq>(y: Wrap<T>) {\n    need_eq(y)\n}\n";
    let no = "fn no<T>(n: Wrap<T>) {\n    need_eq(n)\n}\n";
    for (row, first, second) in [("yes then no", yes, no), ("no then yes", no, yes)] {
        let src = format!("{NEEDS}{wrap}{first}{second}fn main() {{}}");
        codes_row(row, &src, &failures(&src, &["need_eq(n)"]));
    }
}

#[test]
fn generated_eq_of_an_enum_with_an_associated_type_payload_should_follow_the_choices() {
    let decls = "class Node {}\ninterface Plain {\n    type Item\n}\ninterface Source {\n    type Item: Eq\n}\nclass Fine: Plain {\n    type Item = i64\n}\nclass Rough: Plain {\n    type Item = Node\n}\nenum W<S: Plain> {\n    A(S.Item)\n}\n";
    let rows = [
        // The bound of the associated type gives `Eq`.
        (
            "item with a bound",
            "fn a<S: Source>(x: S.Item) {\n    need_eq(x)\n}\n",
            true,
        ),
        (
            "item without a bound",
            "fn a<S: Plain>(x: S.Item) {\n    need_eq(x)\n}\n",
            false,
        ),
        (
            "display of an item",
            "fn a<S: Plain>(x: S.Item) {\n    need_display(x)\n}\n",
            true,
        ),
        // The payload of an enum is an associated type: the choice of the argument decides.
        (
            "enum of an item",
            "fn a<S: Plain>(x: W<S>) {\n    need_eq(x)\n}\n",
            false,
        ),
        (
            "enum of a chosen number",
            "fn a(x: W<Fine>) {\n    need_eq(x)\n}\n",
            true,
        ),
        (
            "enum of a chosen class",
            "fn a(x: W<Rough>) {\n    need_eq(x)\n}\n",
            false,
        ),
    ];
    for (row, function, passes) in rows {
        let src = format!("{NEEDS}{decls}{function}fn main() {{}}");
        if passes {
            clean_row(row, &src);
        } else {
            codes_row(row, &src, &failures(&src, &["need_eq(x)"]));
        }
    }
}

/// `enum A { X(B)\n Y(Node) }` and `enum B { Z(A) }`: neither has `Eq`, whichever is asked first.
#[test]
fn generated_eq_of_enums_that_need_each_other_and_a_class_should_fail_in_both_orders() {
    let decls = "class Node {}\nenum A {\n    X(B)\n    Y(Node)\n}\nenum B {\n    Z(A)\n}";
    for (row, calls) in [
        ("A first", ["need_eq(a)", "need_eq(b)"]),
        ("B first", ["need_eq(b)", "need_eq(a)"]),
    ] {
        let src = calling(decls, "a: A, b: B", &calls);
        codes_row(row, &src, &failures(&src, &calls));
    }
}

/// `enum A { P(B)\n Q(Node) }`, `enum B { P(C)\n Q(A) }` and `enum C { P(B) }`: none has `Eq`,
/// and an answer that rests on a key that fails is never kept as `true`.
#[test]
fn generated_eq_of_three_enums_that_rest_on_a_failing_key_should_fail_in_both_orders() {
    let decls = "class Node {}\nenum A {\n    P(B)\n    Q(Node)\n}\nenum B {\n    P(C)\n    Q(A)\n}\nenum C {\n    P(B)\n}";
    for (row, calls) in [
        ("A first", ["need_eq(a)", "need_eq(b)", "need_eq(c)"]),
        ("C first", ["need_eq(c)", "need_eq(a)", "need_eq(b)"]),
    ] {
        let src = calling(decls, "a: A, b: B, c: C", &calls);
        codes_row(row, &src, &failures(&src, &calls));
        // Each of `A`, `B`, `C` and `Node` is decided once: the root keeps the answers that its
        // failure leaves as `false`, so the calls after the first add no step.
        assert_eq!(steps(&src).0, 4, "{row}");
    }
}

/// A written `Display` bound does not make `Eq` read bounds: `conforms` for `Display` is `true`
/// with no record read.
#[test]
fn generated_eq_under_a_written_display_bound_should_not_read_that_bound() {
    let decls = "class Node {}\nclass Box<T> {\n    v: T\n}\nclass Wr<T> {\n    v: T\n}\nextend<T: Display> Box<T>: Eq {\n    fn eq(self, other: Box<T>) -> bool { true }\n}\nextend<T: Eq> Wr<T>: Display {\n    fn to_string(self) -> string { \"w\" }\n}\nenum H {\n    P(Box<Wr<A>>)\n}\nenum A {\n    X(B)\n    Y(Node)\n}\nenum B {\n    Z(A)\n    M(H)\n}";
    for (row, calls) in [
        ("H first", ["need_eq(h)", "need_eq(b)"]),
        ("B first", ["need_eq(b)", "need_eq(h)"]),
    ] {
        let src = calling(decls, "h: H, b: B", &calls);
        codes_row(row, &src, &failures(&src, &["need_eq(b)"]));
    }
}

/// A written record inside an enum decides with its bounds, which see the enum's arguments.
#[test]
fn generated_eq_of_an_enum_should_ask_the_bounds_of_a_written_record_inside_it() {
    let boxed = "class Box<T> {\n    v: T\n}\n";
    let eq = |bound: &str| {
        format!(
            "extend{bound} Box<T>: Eq {{\n    fn eq(self, other: Box<T>) -> bool {{ true }}\n}}\n"
        )
    };
    let rows: Vec<(&str, String, &str, bool)> = vec![
        (
            "a display bound holds for an array",
            format!("{boxed}{}enum E<U> {{\n    A(Box<[U]>)\n}}\n", eq("<T: Display>")),
            "E<i64>",
            true,
        ),
        (
            "an ord bound holds for a number",
            format!("{boxed}{}enum E {{\n    A(Box<i64>)\n}}\n", eq("<T: Ord>")),
            "E",
            true,
        ),
        (
            "an ord bound fails for an enum, which has no Ord",
            format!(
                "{boxed}{}enum Inner<U> {{\n    A(U)\n}}\nenum E<U> {{\n    A(Box<Inner<U>>)\n}}\n",
                eq("<T: Ord>")
            ),
            "E<i64>",
            false,
        ),
        (
            "a record for one argument holds for it",
            format!(
                "{boxed}extend Box<i64>: Eq {{\n    fn eq(self, other: Box<i64>) -> bool {{ true }}\n}}\nenum E<T> {{\n    A(Box<T>)\n}}\n"
            ),
            "E<i64>",
            true,
        ),
        (
            "a record for one argument fails for another",
            format!(
                "{boxed}extend Box<i64>: Eq {{\n    fn eq(self, other: Box<i64>) -> bool {{ true }}\n}}\nenum E<T> {{\n    A(Box<T>)\n}}\n"
            ),
            "E<string>",
            false,
        ),
        (
            "a class with an ord bound on an enum fails",
            "class C<T> {\n    v: T\n}\nextend<T: Ord> C<T>: Eq {\n    fn eq(self, other: C<T>) -> bool { true }\n}\nenum E {\n    A\n}\nenum H {\n    One(C<E>)\n}\n".to_string(),
            "H",
            false,
        ),
        (
            "a class with an ord bound on a number holds",
            "class C<T> {\n    v: T\n}\nextend<T: Ord> C<T>: Eq {\n    fn eq(self, other: C<T>) -> bool { true }\n}\nenum H {\n    One(C<i64>)\n}\n".to_string(),
            "H",
            true,
        ),
    ];
    for (row, decls, ty, passes) in rows {
        let src = asking(&decls, "eq", ty);
        if passes {
            clean_row(row, &src);
        } else {
            let at = argument(&src, "need_eq(x)");
            codes_row(row, &src, &[(Code::NotConform, at)]);
        }
    }
}

/// The bound of a written record names an interface with an argument, which takes the types that
/// stand for the record's parameters: `Tag<[T]>` for `F<T>`.
#[test]
fn generated_eq_of_an_enum_should_substitute_the_interface_arguments_of_a_record_bound() {
    let decls = |tag: &str| {
        format!(
            "interface Tag<X> {{}}\nclass K {{}}\nextend K: Tag<{tag}> {{}}\nclass Two<A, B> {{\n    a: A\n    b: B\n}}\nextend<A: Tag<B>, B> Two<A, B>: Eq {{\n    fn eq(self, other: Two<A, B>) -> bool {{ true }}\n}}\nenum F<T> {{\n    One(Two<K, [T]>)\n}}"
        )
    };
    clean_row("tag of the array", &asking(&decls("[i64]"), "eq", "F<i64>"));
    let src = asking(&decls("i64"), "eq", "F<i64>");
    codes_row(
        "tag of the element",
        &src,
        &[(Code::NotConform, argument(&src, "need_eq(x)"))],
    );
}

/// A written `Eq` that covers a type decides: when its bounds fail the type has no `Eq`, with no
/// fallback to the generated rule, and the enum that holds it has none either.
#[test]
fn a_written_eq_whose_bound_fails_should_not_fall_back_to_the_generated_rule() {
    let decls = "enum Pair<T> {\n    X(T)\n}\nextend<T: Ord> Pair<T>: Eq {\n    fn eq(self, other: Pair<T>) -> bool { true }\n}\nenum Outer<T> {\n    A(Pair<T>)\n}";
    for (row, calls) in [
        ("pair first", ["need_eq(p)", "need_eq(o)"]),
        ("outer first", ["need_eq(o)", "need_eq(p)"]),
    ] {
        let src = calling(decls, "p: Pair<bool>, o: Outer<bool>", &calls);
        codes_row(row, &src, &failures(&src, &calls));
    }
    // The same enums with a type that has `Ord` hold.
    let src = calling(
        decls,
        "p: Pair<i64>, o: Outer<i64>",
        &["need_eq(p)", "need_eq(o)"],
    );
    clean_row("pair of a number", &src);
}

#[test]
fn a_listed_eq_with_no_method_should_give_the_enum_its_generated_conformance() {
    let wrap = "enum Wrap<T>: Eq {\n    A(T)\n}\nclass Node {}";
    clean_row("number", &asking(wrap, "eq", "Wrap<i64>"));
    let src = asking(wrap, "eq", "Wrap<Node>");
    codes_row(
        "class",
        &src,
        &[(Code::NotConform, argument(&src, "need_eq(x)"))],
    );
}

/// A program of `n` enums.
type Program = fn(usize) -> String;

/// `CONFORMS_STEPS` counts one step for each class or enum key that a query expands.
fn steps(src: &str) -> (usize, Vec<Code>) {
    CONFORMS_STEPS.set(0);
    CONFORMS_BUDGET.set(1_000);
    let (_, ds) = checked(src);
    (
        CONFORMS_STEPS.get(),
        ds.iter().map(Diagnostic::code).collect(),
    )
}

/// `enum C0 { A(C1, C1) }` … `enum C(n-1) { A(Cn, Cn) }` and `enum Cn { End(i64) }`: with the
/// second payload of each a memo hit, `need_eq` on a `C0` takes one step for each enum.
fn chain(n: usize) -> String {
    let mut enums: Vec<String> = (0..n - 1)
        .map(|k| format!("enum C{k} {{\n    A(C{}, C{})\n}}\n", k + 1, k + 1))
        .collect();
    enums.push(format!("enum C{} {{\n    End(i64)\n}}\n", n - 1));
    asking(&enums.concat(), "eq", "C0")
}

/// The enums of the cyclic chain: `Ck` has `End` and `A(C(k+1), C(k+1))`, and the last has
/// `A(C0)`.
fn cycle_enums(n: usize) -> String {
    let mut enums: Vec<String> = (0..n - 1)
        .map(|k| format!("enum C{k} {{\n    End\n    A(C{}, C{})\n}}\n", k + 1, k + 1))
        .collect();
    enums.push(format!("enum C{} {{\n    End\n    A(C0)\n}}\n", n - 1));
    enums.concat()
}

fn cycle(n: usize) -> String {
    asking(&cycle_enums(n), "eq", "C0")
}

/// The cyclic chain of `n` enums under `n` roots `enum Rk { X(C0)\n Y(Node) }`, each asked.
fn roots(n: usize) -> String {
    let decls: Vec<String> = (1..=n)
        .map(|k| format!("enum R{k} {{\n    X(C0)\n    Y(Node)\n}}\n"))
        .collect();
    let decls = format!("class Node {{}}\n{}{}", cycle_enums(n), decls.concat());
    let params: Vec<String> = (1..=n).map(|k| format!("r{k}: R{k}")).collect();
    let calls: Vec<String> = (1..=n).map(|k| format!("need_eq(r{k})")).collect();
    let calls: Vec<&str> = calls.iter().map(String::as_str).collect();
    calling(&decls, &params.join(", "), &calls)
}

/// `enum C0 { A(C1) }` … `enum C(n-1) { End(Node) }` and a class `Node` with no `Eq`, and `n`
/// functions that each compare two values of `C(k)`, where `k` is `at(i, n)` for the function `i`.
/// In a ring, the last enum also has `A(C0)`.
fn comparisons(n: usize, at: fn(usize, usize) -> usize, ring: bool) -> String {
    let mut enums: Vec<String> = (0..n - 1)
        .map(|k| format!("enum C{k} {{\n    A(C{})\n}}\n", k + 1))
        .collect();
    let back = if ring { "    A(C0)\n" } else { "" };
    enums.push(format!("enum C{} {{\n{back}    End(Node)\n}}\n", n - 1));
    let fns: Vec<String> = (0..n)
        .map(|i| {
            let ty = format!("C{}", at(i, n));
            format!("fn f{i}(a: {ty}, b: {ty}) -> bool {{ a == b }}\n")
        })
        .collect();
    format!(
        "class Node {{}}\n{}{}fn main() {{}}",
        enums.concat(),
        fns.concat()
    )
}

/// Each comparison that fails reports the leaf, and a type that was read for one report is not
/// read again for the next.
#[test]
fn failing_leaf_should_read_each_type_once() {
    /// A row: its name, the enum that the function `i` compares, and whether the chain is a ring.
    type Row = (&'static str, fn(usize, usize) -> usize, bool);
    let rows: [Row; 5] = [
        (
            "the root of a chain, compared again and again",
            |_, _| 0,
            false,
        ),
        ("each enum of a chain, from the first", |i, _| i, false),
        (
            "each enum of a chain, from the last",
            |i, n| n - 1 - i,
            false,
        ),
        (
            "the root of a ring, compared again and again",
            |_, _| 0,
            true,
        ),
        ("each member of a ring", |i, _| i, true),
    ];
    for (row, at, ring) in rows {
        let count = |n: usize| {
            LEAF_STEPS.set(0);
            let (_, ds) = checked(&comparisons(n, at, ring));
            assert_eq!(
                ds.iter().filter(|d| d.code() == Code::NoOperator).count(),
                n,
                "row `{row}`"
            );
            LEAF_STEPS.get()
        };
        let (one, two, three) = (count(20), count(40), count(60));
        assert_eq!(
            one, 21,
            "row `{row}`: one read for each enum and for the class"
        );
        assert_near_linear(one, two, three);
    }
}

#[test]
fn generated_should_decide_each_type_once() {
    // Only the steps: each enum of the chain holds two of the next, so the payload limit adds an
    // E0160 that this test is not about.
    assert_eq!(steps(&chain(40)).0, 40);
    // The coinductive hit costs no step of its own.
    let expr = "enum Expr {\n    Num(i64)\n    Add(Expr, Expr)\n}";
    assert_eq!(steps(&asking(expr, "eq", "Expr")), (1, vec![]));
    // A declaration that grows ends its query in one step, and has its own error alone.
    let nest = "enum Nest<T> {\n    End\n    Next(Nest<[T]>)\n}";
    assert_eq!(
        steps(&asking(nest, "eq", "Nest<i64>")),
        (1, vec![Code::InstantiationCycle])
    );
}

#[test]
fn generated_should_decide_each_type_of_a_branching_chain_once() {
    // `Ek<T>` has `A(E(k+1)<[T]>)`, `B(E(k+1)<T?>)` and a repeat of `A`'s payload, so the types
    // formed at level k are 2^k, and the 127 of the 7 levels are all distinct.
    let mut decls: Vec<String> = (0..6)
        .map(|k| {
            format!(
                "enum E{k}<T> {{\n    A(E{n}<[T]>)\n    B(E{n}<T?>)\n    C(E{n}<[T]>)\n}}\n",
                n = k + 1
            )
        })
        .collect();
    decls.push("enum E6<T> {\n    A(T)\n}\n".to_string());
    let decls = decls.concat();
    let src = calling(&decls, "x: E0<i64>", &["need_eq(x)"]);
    assert_eq!(steps(&src), (127, vec![]));
}

#[test]
fn pending_answers_should_expand_each_key_once() {
    assert_eq!(steps(&cycle(40)), (40, vec![]));
}

#[test]
fn failing_roots_should_keep_the_answers_of_a_cycle_below_them() {
    let src = roots(40);
    let (count, codes) = steps(&src);
    assert_eq!(count, 81);
    assert_eq!(codes, vec![Code::NotConform; 40]);
}

#[test]
fn queries_should_cost_steps_in_proportion_to_the_types_they_form() {
    let programs: [(&str, Program); 3] = [
        ("chain of enums", chain),
        ("cycle of enums", cycle),
        ("roots that fail", roots),
    ];
    for (row, program) in programs {
        let count = |n: usize| {
            CONFORMS_STEPS.set(0);
            CONFORMS_BUDGET.set(100_000);
            checked(&program(n));
            CONFORMS_STEPS.get()
        };
        let (one, two, three) = (count(20), count(40), count(60));
        assert!(one > 0, "{row}");
        assert_near_linear(one, two, three);
    }
}

/// A valid chain or ring of enums, longer than the old limit of 1024 open queries, is clean: the
/// query for `==` on its first enum opens one key for each enum, and none of them is an endless
/// instantiation (#247).
#[test]
fn a_chain_or_ring_of_enums_past_the_old_limit_should_check_clean() {
    CONFORMS_BUDGET.set(100_000);
    let main = "fn main() {\n    let e: C0 = C0.End\n    print(e == e)\n}\n";
    let chain: String = (0..2000)
        .map(|k| match k {
            1999 => format!("enum C{k} {{\n    End\n}}\n"),
            _ => format!("enum C{k} {{\n    End\n    A(C{})\n}}\n", k + 1),
        })
        .collect();
    clean_row("a chain of 2000", &format!("{chain}{main}"));
    // The ring closes with `A(C0)`, so a query for `C0` meets itself after 2000 keys.
    let ring = chain.replace(
        "enum C1999 {\n    End\n}",
        "enum C1999 {\n    End\n    A(C0)\n}",
    );
    clean_row("a ring of 2000", &format!("{ring}{main}"));
}

/// `class W<T>` with a written `Eq` whose bound asks `T`, and a probe type that gives `W` its id.
const WRAPPER: &str = "class W<T> {\n    value: T\n}\nextend<T: Eq> W<T>: Eq {\n    fn eq(self, other: Self) -> bool {\n        self.value == other.value\n    }\n}\n";

/// The id of the class named `name`.
fn class_named(checker: &Checker<'_>, name: &str) -> tir::ClassId {
    let id = (checker.classes.iter()).position(|class| class.name.name.as_str() == name);
    tir::ClassId(id.expect("the program should declare the class"))
}

/// The class or enum type named `holder` with `inner` as its one argument, `levels` times over:
/// `W<W<...<inner>...>>`. With `levels` zero, `inner`.
fn nested(checker: &Checker<'_>, holder: &str, levels: usize, inner: Type) -> Type {
    let class = (checker.classes.iter()).position(|class| class.name.name.as_str() == holder);
    let enum_id = (checker.enums.iter()).position(|e| e.name.name.as_str() == holder);
    let kind = |args| match (class, enum_id) {
        (Some(id), _) => TypeKind::Class(tir::ClassId(id), args),
        (None, Some(id)) => TypeKind::Enum(tir::EnumId(id), args),
        (None, None) => panic!("the program should declare `{holder}`"),
    };
    (0..levels).fold(inner, |ty, _| Type::intern(kind(TypeArgs::intern(&[ty]))))
}

/// What `checker.conforms` says for `W` nested `levels` times over `inner`. Each query uses its own
/// innermost type, so that no answer kept for one is read for the next.
fn query_nest(checker: &mut Checker<'_>, eq: tir::InterfaceId, levels: usize, inner: Type) -> bool {
    let ty = nested(checker, "W", levels, inner);
    checker.conforms(ty, eq, TypeArgs::NONE)
}

/// A query for a conformance holds `CONFORMS_DEPTH_LIMIT` class or enum keys open at most, one
/// inside the other. A type that nests as many `W` as the limit is decided with no report: a
/// class counts, as an enum does, because the walk of a written `Eq` goes through the bound of
/// each level. The test lowers the limit, as a walk of the whole limit takes about a second.
#[test]
fn a_query_should_be_clean_at_the_depth_limit() {
    let limit = 600;
    CONFORMS_DEPTH.set(limit);
    CONFORMS_BUDGET.set(2 * limit);
    with_probe(WRAPPER, ("T", "W<i64>"), "Eq", |checker, _, eq| {
        assert!(query_nest(checker, eq, limit, Type::BOOL));
        assert!(checker.diagnostics.is_empty());
    });
}

/// One `W` more than the limit makes the query stop and report E0140 at the name of the class
/// that it reached. The stopped query holds: the report is its only diagnostic. The test lowers
/// the limit; the constant above pins the real one.
#[test]
fn a_query_should_report_the_class_it_reached_past_the_depth_limit() {
    let limit = 600;
    CONFORMS_DEPTH.set(limit);
    CONFORMS_BUDGET.set(2 * limit);
    let (name, found) = with_probe(WRAPPER, ("T", "W<i64>"), "Eq", |checker, _, eq| {
        assert!(query_nest(checker, eq, limit + 1, Type::I64));
        let found: Vec<_> = (checker.diagnostics.iter())
            .map(|d| (d.code(), d.span()))
            .collect();
        (
            checker.classes[class_named(checker, "W").0].name.span,
            found,
        )
    });
    assert_eq!(found, [(Code::InstantiationCycle, name)]);
}

/// The report of a query that is too deep names the interface, the type that the query began
/// with, the limit and the class that it reached.
#[test]
fn a_query_that_is_too_deep_should_fill_each_field_of_its_report() {
    CONFORMS_DEPTH.set(3);
    CONFORMS_BUDGET.set(100);
    let (found, at) = with_probe(WRAPPER, ("T", "W<i64>"), "Eq", |checker, _, eq| {
        assert!(query_nest(checker, eq, 4, Type::I64));
        (
            (checker.diagnostics.iter())
                .map(|d| format!("{d:?}"))
                .collect::<Vec<_>>(),
            checker.classes[class_named(checker, "W").0].name.span,
        )
    });
    // `Diagnostic` derives only `Clone` and `Debug`, so the test compares rendered text. The
    // registry arm that builds the message and the label uses every field of the kind.
    let expected: Diagnostic = DiagnosticKind::ConformanceTooDeep {
        interface: "Eq".to_string(),
        root: "W<W<W<W<i64>>>>".to_string(),
        limit: 3,
        holder: "W".to_string(),
        at,
    }
    .into();
    assert_eq!(found, [format!("{expected:?}")]);
}

/// A second root that goes past the limit through the same class reports nothing more.
#[test]
fn a_query_should_report_a_class_past_the_depth_limit_once() {
    let limit = 600;
    CONFORMS_DEPTH.set(limit);
    CONFORMS_BUDGET.set(4 * limit);
    let found = with_probe(WRAPPER, ("T", "W<i64>"), "Eq", |checker, _, eq| {
        assert!(query_nest(checker, eq, limit + 1, Type::I64));
        assert!(query_nest(checker, eq, limit + 1, Type::CHAR));
        checker.diagnostics.len()
    });
    assert_eq!(found, 1);
}

/// A class or an enum that grows has its own error (E0140), so a query that reaches it past the
/// limit gives no second report: a query that holds `W` nested as the limit allows, or one more,
/// over a growing `G` ends with the one error of `G`, at the same place (#242).
#[test]
fn a_query_that_reaches_a_growing_declaration_past_the_depth_limit_should_report_it_once() {
    let limit = 600;
    CONFORMS_DEPTH.set(limit);
    // Four programs, each about `limit` steps.
    CONFORMS_BUDGET.set(8 * limit);
    let class = "class G<T> {\n    next: G<[T]>?\n}\nextend<T: Eq> G<T>: Eq {\n    fn eq(self, other: Self) -> bool {\n        true\n    }\n}\n";
    let rows = [
        ("enum", "enum G<T> {\n    End\n    A(G<[T]>)\n}\n", (11, 7)),
        ("class", class, (10, 11)),
    ];
    for (kind, decl, at) in rows {
        for levels in [limit, limit + 1] {
            let mut lets = String::new();
            for i in 1..=levels {
                writeln!(lets, "    let a{i} = W(value: a{})", i - 1)
                    .expect("writing to a `String` should succeed");
            }
            let src = format!(
                "{WRAPPER}{decl}fn main() {{\n    let a0: G<i64>? = None\n{lets}    print(a{levels} == a{levels})\n}}"
            );
            codes_row(
                &format!("{levels} levels of `W` over a growing {kind}"),
                &src,
                &[(Code::InstantiationCycle, at)],
            );
        }
    }
}

/// The walk that names the part of a failed query stops where the query did: below an enum whose
/// query was too deep, a part fails that the query never read, and the part that made the query
/// fail is the one to name. Here `O` nests as deep as the limit over `Node`, which has no `Eq`, and
/// `Shallow` is the payload of `R` that fails the query.
#[test]
fn failing_leaf_should_stop_at_an_enum_that_a_query_was_too_deep_for() {
    let limit = 600;
    CONFORMS_DEPTH.set(limit);
    CONFORMS_BUDGET.set(2 * limit);
    let decls = "class Node {}\nclass Shallow {}\nenum O<T> {\n    End\n    S(T)\n}\nenum R<T> {\n    A(T)\n    B(Shallow)\n}\n";
    let leaf = with_probe(decls, ("T", "R<i64>"), "Eq", |checker, _, eq| {
        let node = Type::intern(TypeKind::Class(
            class_named(checker, "Node"),
            TypeArgs::NONE,
        ));
        let deep = nested(checker, "O", limit, node);
        let ty = nested(checker, "R", 1, deep);
        assert!(!checker.conforms(ty, eq, TypeArgs::NONE), "`Shallow` fails");
        let leaf = checker.failing_leaf(ty, eq, TypeArgs::NONE);
        checker.type_name(leaf)
    });
    assert_eq!(leaf, "Shallow");
}

/// The part that makes a failed query fail is found on a stack of 64 KiB, for a chain of enums
/// that nests deeper than that stack allows recursing into (#242). `failing_leaf` follows the
/// chain one level at a time, so it grows the stack as the query does.
#[test]
fn failing_leaf_should_follow_a_deep_chain_on_a_small_stack() {
    const STACK: usize = 64 * 1024;
    // With no stack guard in `leaf`, 39 levels abort on this stack and 38 do not: 400 levels
    // are about ten times that.
    let levels = 400;
    let leaf = std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            let decls = "class Node {}\nenum O<T> {\n    End\n    S(T)\n}\n";
            with_probe(decls, ("T", "O<i64>"), "Eq", |checker, _, eq| {
                let node = Type::intern(TypeKind::Class(
                    class_named(checker, "Node"),
                    TypeArgs::NONE,
                ));
                let ty = nested(checker, "O", levels, node);
                assert!(!checker.conforms(ty, eq, TypeArgs::NONE), "`Node` fails");
                let leaf = checker.failing_leaf(ty, eq, TypeArgs::NONE);
                checker.type_name(leaf)
            })
        })
        .expect("the thread should start")
        .join()
        .expect("the walk should not panic");
    assert_eq!(leaf, "Node");
}

/// A declaration that grows or that no value can be built of has its own error, and the query
/// for it ends in one step with no part query, so nothing else reports it.
#[test]
fn growing_and_never_built_declarations_should_give_their_own_error_alone() {
    let nest = "enum Nest<T> {\n    End\n    Next(Nest<[T]>)\n}\n";
    for (row, src, expected) in [
        (
            "an enum that no value can be built of",
            "class Node {}\nenum Dead {\n    Again(Dead, Node)\n}\nclass NeedsEq<T: Eq> {\n    value: T\n}\nfn use(x: NeedsEq<Dead>) {}\nfn main() {}"
                .to_string(),
            vec![(Code::NeverConstructed, (2, 6))],
        ),
        (
            "a growing enum that lists Eq",
            "enum Nest<T>: Eq {\n    End\n    Next(Nest<[T]>)\n}\nfn main() {}".to_string(),
            vec![(Code::InstantiationCycle, (3, 10))],
        ),
        (
            // The growth is through the choice of an associated type, which a conformance
            // records: the enum is marked once the choices are known, before the first query.
            "an enum that grows through an associated type",
            format!(
                "interface Seq {{\n    type Item: Seq\n}}\nclass Wrap<T> {{\n    v: T\n}}\nextend<T: Seq> Wrap<T>: Seq {{\n    type Item = Wrap<Wrap<T>>\n}}\nclass Start {{}}\nextend Start: Seq {{\n    type Item = Wrap<Start>\n}}\nenum Grow<S: Seq> {{\n    End\n    Next(Grow<S.Item>)\n}}\n{NEEDS}fn f(x: Grow<Start>) {{\n    need_eq(x)\n}}\nfn main() {{}}"
            ),
            vec![(Code::InstantiationCycle, (16, 10))],
        ),
        (
            "a growing enum that a dbg shows",
            format!(
                "{nest}fn show<T>(x: Nest<T>) {{\n    dbg(x)\n}}\nfn main() {{\n    let n: Nest<i64> = Nest.End\n    show(n)\n}}"
            ),
            vec![(Code::InstantiationCycle, (3, 10))],
        ),
        (
            "a growing enum under the bound of a class",
            format!("class C<T: Eq> {{}}\n{nest}enum U {{\n    A(C<Nest<i64>>)\n}}\nfn main() {{}}"),
            vec![(Code::InstantiationCycle, (4, 10))],
        ),
    ] {
        codes_row(row, &src, &expected);
    }
}

/// An enum that holds the type that a conformance chooses for `T.Item`, and a choice that names
/// the enum again with a longer type argument, grows without end: the enum is marked before the
/// first query, and the choice is reported (E0140). The three forms below overflowed the stack.
#[test]
fn growth_through_the_choice_of_an_associated_type_should_be_marked_before_the_first_query() {
    let decls = "interface I {\n    type Item\n}\nclass W<T>: I {\n    type Item = E<W<W<T>>>\n}\nenum E<T: I> {\n    End\n    A(T.Item)\n}\n";
    let rows = [
        (
            "a listed Eq on an enum that holds the grown enum",
            "enum F: Eq {\n    X(E<W<i64>>)\n}\nfn main() {}",
        ),
        (
            "a comparison of the grown enum",
            "fn main() {\n    let e: E<W<i64>> = E.End\n    print(e == e)\n}",
        ),
        (
            "a call of a bound that the grown enum must meet",
            "fn f<T: Eq>(_x: T) {}\nfn main() {\n    let e: E<W<i64>> = E.End\n    f(e)\n}",
        ),
        (
            "a value of the grown enum and no query",
            "fn main() {\n    let e: E<W<i64>> = E.End\n}",
        ),
    ];
    for (row, rest) in rows {
        let src = format!("{decls}{rest}");
        codes_row(
            row,
            &src,
            &[(Code::InstantiationCycle, where_is(&src, "E<W<W<T>>>"))],
        );
    }
}

/// A use of an associated type feeds one hub, and the hub feeds each choice of it, so the graph
/// holds the edges of the uses and of the choices added together, not multiplied.
#[test]
fn uses_and_choices_of_an_associated_type_should_cost_edges_in_proportion_to_their_sum() {
    /// What the program has `n` of.
    #[derive(Clone, Copy)]
    enum Shape {
        /// Choices of one interface, and uses of it.
        One,
        /// Choices and uses of an interface with one list of arguments that holds no parameter,
        /// and one that does.
        Arguments,
        /// Choices for `n` different lists of arguments that hold no parameter, and uses whose
        /// arguments hold a parameter: each use meets every list, so a use that fed each list
        /// itself would cost the product.
        Lists,
    }
    let edges = |n: usize, shape: Shape| {
        let arguments = matches!(shape, Shape::Arguments);
        let (interface, bound) = if arguments {
            ("S<A> {\n    type Item\n}\n", "S<i64>")
        } else {
            ("I {\n    type Item\n}\n", "I")
        };
        let mut src = if matches!(shape, Shape::Lists) {
            "interface S<A> {\n    type Item\n}\n".to_string()
        } else {
            format!("interface {interface}")
        };
        if matches!(shape, Shape::Lists) {
            for k in 0..n {
                writeln!(
                    src,
                    "class C{k} {{}}\nclass W{k}<T>: S<C{k}> {{\n    type Item = T\n}}"
                )
                .expect("writing to a `String` should succeed");
            }
            for j in 0..n {
                writeln!(src, "class G{j}<T: S<A>, A> {{\n    v: T.Item?\n}}")
                    .expect("writing to a `String` should succeed");
            }
        } else {
            for k in 0..n {
                writeln!(src, "class W{k}<T>: {bound} {{\n    type Item = T\n}}")
                    .expect("writing to a `String` should succeed");
                if arguments {
                    // A conformance with a parameter in its interface arguments.
                    writeln!(src, "class V{k}<T, U>: S<U> {{\n    type Item = T\n}}")
                        .expect("writing to a `String` should succeed");
                }
            }
            writeln!(src, "enum E<T: {bound}> {{\n    End\n    A(T.Item)\n}}")
                .expect("writing to a `String` should succeed");
            for j in 0..n {
                writeln!(src, "class H{j}<T: {bound}> {{\n    v: T.Item?\n}}")
                    .expect("writing to a `String` should succeed");
                if arguments {
                    // A use with a parameter in its interface arguments.
                    writeln!(src, "class G{j}<T: S<A>, A> {{\n    v: T.Item?\n}}")
                        .expect("writing to a `String` should succeed");
                }
            }
        }
        src += "fn main() {}";
        cycle::EDGES.set(0);
        clean(&src);
        cycle::EDGES.get()
    };
    for (row, shape) in [
        ("one interface", Shape::One),
        ("interface arguments", Shape::Arguments),
        ("many lists of interface arguments", Shape::Lists),
    ] {
        let (one, two, three) = (edges(50, shape), edges(100, shape), edges(150, shape));
        assert!(one > 0, "{row}");
        assert_near_linear(one, two, three);
    }
}

/// The neighbours of the forms above: each holder of the associated type, each way to name it,
/// and each shape of choice that grows.
#[test]
fn growth_through_the_choice_of_an_associated_type_should_be_reported_for_each_form() {
    let item = "interface I {\n    type Item\n}\n";
    let chain = "interface I {\n    type Item\n}\ninterface Nx {\n    type Next: I\n}\n";
    let list = "enum F: Eq {\n    X(E<W<i64>>)\n}\nfn main() {}";
    // Each row: the program, and the text of the choice where the error stands.
    let rows = [
        (
            "a class holds the associated type",
            format!("{item}class W<T>: I {{\n    type Item = H<W<W<T>>>\n}}\nclass H<T: I> {{\n    v: T.Item?\n}}\nfn main() {{}}"),
            "H<W<W<T>>>",
        ),
        (
            "an enum is the target of the conformance",
            format!("{item}enum W<T>: I {{\n    One(T)\n    type Item = E<W<W<T>>>\n}}\nenum E<T: I> {{\n    End\n    A(T.Item)\n}}\nfn main() {{}}"),
            "E<W<W<T>>>",
        ),
        (
            "an array of the associated type",
            format!("{item}class W<T>: I {{\n    type Item = E<W<W<T>>>\n}}\nenum E<T: I> {{\n    End\n    A([T.Item])\n}}\n{list}"),
            "E<W<W<T>>>",
        ),
        (
            "an argument of another enum",
            format!("{item}class W<T>: I {{\n    type Item = E<W<W<T>>>\n}}\nenum Box<U> {{\n    B(U)\n}}\nenum E<T: I> {{\n    End\n    A(Box<T.Item>)\n}}\n{list}"),
            "E<W<W<T>>>",
        ),
        (
            "a choice with two parameters",
            format!("{item}class P<A, B>: I {{\n    type Item = E<P<B, P<A, A>>>\n}}\nenum E<T: I> {{\n    End\n    A(T.Item)\n}}\nenum F: Eq {{\n    X(E<P<i64, i64>>)\n}}\nfn main() {{}}"),
            "E<P<B, P<A, A>>>",
        ),
        (
            // The type grows through the second parameter alone: `B` is nested one level deeper
            // than `A`, which is what the edge out of the gate to `B` must carry.
            "a choice with two parameters that grows through the second one",
            format!("{item}class P<A, B>: I {{\n    type Item = E<P<A, P<B, B>>>\n}}\nenum E<T: I> {{\n    End\n    A(T.Item)\n}}\nenum F: Eq {{\n    X(E<P<i64, i64>>)\n}}\nfn main() {{}}"),
            "E<P<A, P<B, B>>>",
        ),
        (
            "an interface with a type parameter",
            "interface S<A> {\n    type Item\n}\nclass W<T>: S<i64> {\n    type Item = E<W<W<T>>>\n}\nenum E<T: S<i64>> {\n    End\n    A(T.Item)\n}\nfn main() {}".to_string(),
            "E<W<W<T>>>",
        ),
        (
            "one conformance grows and another shrinks",
            format!("{item}class W<T>: I {{\n    type Item = T\n}}\nclass V<T>: I {{\n    type Item = E<V<V<T>>>\n}}\nenum E<T: I> {{\n    End\n    A(T.Item)\n}}\nenum F: Eq {{\n    X(E<W<i64>>)\n}}\nfn main() {{}}"),
            "E<V<V<T>>>",
        ),
        (
            "two enums hold the associated type and each other",
            format!("{item}class W<T>: I {{\n    type Item = G<W<W<T>>>\n}}\nenum E<T: I> {{\n    End\n    A(T.Item)\n}}\nenum G<T: I> {{\n    End\n    B(E<T>)\n}}\n{list}"),
            "G<W<W<T>>>",
        ),
        (
            "an associated type of an associated type",
            format!("{chain}class X<T>: Nx {{\n    type Next = W<T>\n}}\nclass W<T>: I {{\n    type Item = E<X<X<T>>>\n}}\nenum E<T: Nx> {{\n    End\n    A(T.Next.Item)\n}}\nenum F: Eq {{\n    Y(E<X<i64>>)\n}}\nfn main() {{}}"),
            "E<X<X<T>>>",
        ),
        (
            // The choice of `Item` passes its parameter on as it is: the type grows by the choice
            // of `Next` alone, so the cycle shows at the field.
            "an associated type of an associated type that grows by its base alone",
            format!("{chain}class X<T>: Nx {{\n    type Next = W<X<X<T>>>\n}}\nclass W<T: Nx>: I {{\n    type Item = E<T>\n}}\nenum E<T: Nx> {{\n    End\n    A(T.Next.Item)\n}}\nenum F: Eq {{\n    Y(E<X<i64>>)\n}}\nfn main() {{}}"),
            "T.Next.Item",
        ),
    ];
    for (row, src, at) in rows {
        codes_row(row, &src, &[(Code::InstantiationCycle, where_is(&src, at))]);
    }
}

/// A choice that a conformance makes for interface arguments that the use may have keeps its
/// edge: the arguments of the use or of the conformance hold a parameter, or are the same. The
/// rows are the neighbours of the ones that `choice_of_an_associated_type_that_does_not_grow`
/// accepts.
#[test]
fn growth_through_a_choice_that_may_be_meant_should_be_reported_for_each_form() {
    let rows = [
        (
            "the same interface arguments, and a second conformance for other arguments",
            format!("{S}class W<T>: S<bool> {{\n    type Item = E<W<W<T>>>\n}}\nextend<T> W<T>: S<i64> {{\n    type Item = i64\n}}\nenum E<T: S<bool>> {{\n    End\n    A(T.Item)\n}}\nfn main() {{}}"),
            "E<W<W<T>>>",
        ),
        (
            "two interface arguments, both the same",
            "interface S<A, B> {\n    type Item\n}\nclass W<T>: S<i64, bool> {\n    type Item = E<W<W<T>>>\n}\nenum E<T: S<i64, bool>> {\n    End\n    A(T.Item)\n}\nfn main() {}".to_string(),
            "E<W<W<T>>>",
        ),
        (
            "a use whose interface arguments are a parameter",
            format!("{S}class W<T>: S<i64> {{\n    type Item = E<W<W<T>>, i64>\n}}\nenum E<T: S<A>, A> {{\n    End\n    A(T.Item)\n}}\nfn main() {{}}"),
            "E<W<W<T>>, i64>",
        ),
        (
            "a class, and a use whose interface arguments are a parameter",
            format!("{S}class W<T>: S<i64> {{\n    type Item = H<W<W<T>>, i64>\n}}\nclass H<T: S<A>, A> {{\n    v: T.Item?\n}}\nfn main() {{}}"),
            "H<W<W<T>>, i64>",
        ),
        (
            "a choice whose conformance has a parameter in its interface arguments",
            format!("{S}class W<T>: S<[T]> {{\n    type Item = E<W<W<T>>, [W<T>]>\n}}\nenum E<T: S<A>, A> {{\n    End\n    A(T.Item)\n}}\nfn main() {{}}"),
            "E<W<W<T>>, [W<T>]>",
        ),
        (
            // The use has the arguments `bool`, which a conformance for `bool` has too, but the
            // choice is made for `S<T>`, which may be `S<bool>`: it stays, as an approximation.
            "a choice for a parameter, beside a conformance for the arguments of the use",
            format!("{S}class V<T>: S<bool> {{\n    type Item = i64\n}}\nclass W<T>: S<T> {{\n    type Item = E<V<V<T>>>\n}}\nenum E<T: S<bool>> {{\n    End\n    A(T.Item)\n}}\nfn main() {{}}"),
            "E<V<V<T>>>",
        ),
        (
            "a choice for a parameter, beside a conformance for the arguments of the use that chooses",
            format!("{S}class V<T>: S<bool> {{\n    type Item = T\n}}\nclass W<T>: S<T> {{\n    type Item = E<V<V<T>>>\n}}\nenum E<T: S<bool>> {{\n    End\n    A(T.Item)\n}}\nfn main() {{}}"),
            "E<V<V<T>>>",
        ),
        (
            "a use with a parameter in its arguments, and a choice for a parameter beside one for a type",
            format!("{S}class V<T>: S<i64> {{\n    type Item = T\n}}\nclass W<T>: S<[T]> {{\n    type Item = E<W<W<T>>, [W<T>]>\n}}\nenum E<T: S<A>, A> {{\n    End\n    A(T.Item)\n}}\nfn main() {{}}"),
            "E<W<W<T>>, [W<T>]>",
        ),
        (
            "a use with a parameter in its arguments, and two conformances for types",
            format!("{S}class V<T>: S<i64> {{\n    type Item = T\n}}\nclass U<T>: S<bool> {{\n    type Item = E<U<U<T>>, bool>\n}}\nenum E<T: S<A>, A> {{\n    End\n    A(T.Item)\n}}\nfn main() {{}}"),
            "E<U<U<T>>, bool>",
        ),
        (
            // The growth of a choice for a parameter holds for the arguments of the use too.
            "an enum that holds the enum of the associated type, and a choice for a parameter",
            "interface S<A> {\n    type Item: S<bool>\n}\nclass V<T>: S<bool> {\n    type Item = V<T>\n}\nclass W<T>: S<T> {\n    type Item = V<V<T>>\n}\nenum E<T: S<bool>> {\n    End\n    A(E<T.Item>)\n}\nfn main() {}".to_string(),
            "E<T.Item>",
        ),
        (
            // The use is for `S<bool>`, and a conformance for `S<bool>` exists, but the choice of
            // the class that the link names is made for `S<T>`, which may be `S<bool>`.
            "a link, and a choice for a parameter beside a conformance for the arguments of the use",
            "interface S<A> {\n    type Link: S<A>\n    type Item\n}\nclass V<T>: S<bool> {\n    type Link = W<bool>\n    type Item = T\n}\nclass W<T>: S<T> {\n    type Link = W<T>\n    type Item = E<V<V<T>>>\n}\nenum E<T: S<bool>> {\n    End\n    A(T.Link.Item)\n}\nfn main() {}".to_string(),
            "E<V<V<T>>>",
        ),
        (
            "an enum that holds the enum of the associated type, for the same arguments",
            format!("{PARTS}extend<T> W<T>: S<i64> {{\n    type Item = W<T>\n}}\nextend<T> W<T>: S<bool> {{\n    type Item = W<W<T>>\n}}\n{PARTS_ENUM}"),
            "E<T.Item>",
        ),
    ];
    for (row, src, at) in rows {
        codes_row(row, &src, &[(Code::InstantiationCycle, where_is(&src, at))]);
    }
}

/// The use `T.Link.Item` means the choices of `Item` that the classes and enums named by the
/// choices of `Link` make. Where a choice of `Link` names a parameter or an associated type, those
/// classes are not known, and where the use has an interface argument that holds a parameter, any
/// choice of `Item` may be meant: the use then keeps the edge to every choice. Each row is a true
/// cycle that a use which drops one of these edges would miss.
#[test]
fn growth_through_a_link_that_names_no_class_should_be_reported_for_each_form() {
    let rows = [
        (
            "a link that one class chooses as a parameter",
            "interface S {\n    type Link: S\n    type Item\n}\nclass Z: S {\n    type Link = Z\n    type Item = i64\n}\nclass V<T: S>: S {\n    type Link = T\n    type Item = i64\n}\nclass W<T: S>: S {\n    type Link = Z\n    type Item = E<V<W<W<T>>>>\n}\nenum E<T: S> {\n    End\n    A(T.Link.Item)\n}\nfn main() {\n    let e: E<V<W<Z>>> = E.End\n    print(e == e)\n}\n",
            "E<V<W<W<T>>>>",
        ),
        (
            "a link that one class chooses as an associated type",
            "interface S {\n    type Link: S\n    type Alt: S\n    type Item\n}\nclass Z: S {\n    type Link = Z\n    type Alt = Z\n    type Item = i64\n}\nclass V<T: S>: S {\n    type Link = T.Alt\n    type Alt = Z\n    type Item = i64\n}\nclass W<T: S>: S {\n    type Link = Z\n    type Alt = W<T>\n    type Item = E<V<W<W<T>>>>\n}\nenum E<T: S> {\n    End\n    A(T.Link.Item)\n}\nfn main() {\n    let e: E<V<W<Z>>> = E.End\n    print(e == e)\n}\n",
            "E<V<W<W<T>>>>",
        ),
        (
            "a use whose interface argument is a parameter",
            "interface S<A> {\n    type Link: S<A>\n    type Item\n}\nclass W<T>: S<bool> {\n    type Link = W<T>\n    type Item = E<W<W<T>>, bool>\n}\nenum E<T: S<U>, U> {\n    End\n    A(T.Link.Item)\n}\nfn main() {\n    let e: E<W<i64>, bool> = E.End\n    print(e == e)\n}\n",
            "E<W<W<T>>, bool>",
        ),
    ];
    for (row, src, at) in rows {
        codes_row(row, src, &[(Code::InstantiationCycle, where_is(src, at))]);
    }
}

/// An interface with an associated type `Next` that one conformance grows without end (`U`),
/// beside the conformance `Z` that does not. A base `T.Next` is then of unbounded growth for the
/// graph, which stands for the choices of every conformance, whatever type it is for.
const UNBOUNDED: &str = "interface S {\n    type Next: S\n    type Item\n}\nclass Z: S {\n    type Next = Z\n    type Item = i64\n}\nclass U<T: S>: S {\n    type Next = U<U<T.Next>>\n    type Item = i64\n}\n";

/// A class `W` that grows the enum `E` that holds `T.Next.Item`, beside `UNBOUNDED`.
const GROWS_BESIDE_UNBOUNDED: &str = "class W<T>: S {\n    type Next = W<T>\n    type Item = E<W<W<T>>>\n}\nenum E<T: S> {\n    End\n    A(T.Next.Item)\n}\n";

/// A base whose associated type is of unbounded growth for some type (`U`) does not make the uses
/// of a type that is not `U` safe: the choice of `W`, a finite class, grows. Each of these
/// programs overflowed the stack of the check, or was accepted and never ended in `sisu build`.
#[test]
fn growth_beside_a_base_of_unbounded_growth_should_be_reported_for_each_form() {
    let compared = "fn main() {\n    let e: E<W<i64>> = E.End\n    print(e == e)\n}";
    let built = "fn main() {\n    let e: E<W<i64>> = E.End\n    print(1)\n}";
    let rows = [
        (
            "the growth is of one list of interface arguments, the use is of another",
            "interface S<A> {\n    type Next: S<A>\n    type Item\n}\nclass Z: S<i64> {\n    type Next = Z\n    type Item = i64\n}\nclass U<T: S<i64>>: S<i64> {\n    type Next = U<U<T.Next>>\n    type Item = i64\n}\nclass W<T>: S<bool> {\n    type Next = W<T>\n    type Item = E<W<W<T>>>\n}\nenum E<T: S<bool>> {\n    End\n    A(T.Next.Item)\n}\nfn main() {\n    let e: E<W<i64>> = E.End\n    print(e == e)\n}".to_string(),
            "E<W<W<T>>>",
        ),
        (
            "the same interface and no arguments, compared",
            format!("{UNBOUNDED}{GROWS_BESIDE_UNBOUNDED}{compared}"),
            "E<W<W<T>>>",
        ),
        (
            "the same, and no query for the value",
            format!("{UNBOUNDED}{GROWS_BESIDE_UNBOUNDED}{built}"),
            "E<W<W<T>>>",
        ),
        (
            "a chain of three links, two of them associated types of unbounded growth",
            format!("{UNBOUNDED}class W<T>: S {{\n    type Next = W<T>\n    type Item = E<W<W<T>>>\n}}\nenum E<T: S> {{\n    End\n    A(T.Next.Next.Next.Item)\n}}\n{compared}"),
            "E<W<W<T>>>",
        ),
        (
            "a link of unbounded growth beside a link that is bounded",
            "interface S {\n    type Next: S\n    type Other: S\n    type Item\n}\nclass Z: S {\n    type Next = Z\n    type Other = Z\n    type Item = i64\n}\nclass U<T: S>: S {\n    type Next = U<U<T.Next>>\n    type Other = U<T>\n    type Item = i64\n}\nclass W<T>: S {\n    type Next = W<T>\n    type Other = W<T>\n    type Item = E<W<W<T>>>\n}\nenum E<T: S> {\n    End\n    A(T.Next.Other.Item)\n}\nfn main() {\n    let e: E<W<i64>> = E.End\n    print(1)\n}".to_string(),
            "E<W<W<T>>>",
        ),
        (
            // The choice of `Other` grows the type by one, and `Item` keeps it: only the link
            // outside the one of unbounded growth makes the cycle.
            "a link that grows beside a link of unbounded growth",
            "interface S {\n    type Next: S\n    type Other: S\n    type Item\n}\nclass Z: S {\n    type Next = Z\n    type Other = Z\n    type Item = i64\n}\nclass U<T: S>: S {\n    type Next = U<U<T.Next>>\n    type Other = U<T>\n    type Item = i64\n}\nclass W<T: S>: S {\n    type Next = W<T>\n    type Other = W<W<T>>\n    type Item = E<W<T>>\n}\nenum E<T: S> {\n    End\n    A(T.Next.Other.Item)\n}\nfn main() {\n    let e: E<W<Z>> = E.End\n    print(1)\n}".to_string(),
            "E<W<T>>",
        ),
        (
            "a class holds the associated type",
            format!("{UNBOUNDED}class W<T>: S {{\n    type Next = W<T>\n    type Item = H<W<W<T>>>\n}}\nclass H<T: S> {{\n    v: T.Next.Item?\n}}\nfn f(_h: H<W<i64>>) {{}}\nfn main() {{}}"),
            "H<W<W<T>>>",
        ),
    ];
    for (row, src, at) in rows {
        codes_row(row, &src, &[(Code::InstantiationCycle, where_is(&src, at))]);
    }
}

/// An interface with a type parameter, and a class whose conformance to it for `i64` grows the
/// type that it chooses, with a conformance of the same class for `bool` that chooses `i64`.
const S: &str = "interface S<A> {\n    type Item\n}\n";
const OTHER_ARGUMENTS: &str = "class W<T>: S<i64> {\n    type Item = E<W<W<T>>>\n}\nextend<T> W<T>: S<bool> {\n    type Item = i64\n}\n";

/// The older form of the same: an associated type that is bound by the interface itself, so that
/// the enum can hold the enum of it. The choices for each class follow, and then `PARTS_ENUM`.
const PARTS: &str = "interface S<A> {\n    type Item: S<bool>\n}\nclass Z {}\nextend Z: S<bool> {\n    type Item = Z\n}\nextend Z: S<i64> {\n    type Item = Z\n}\nclass W<T> {}\n";
const PARTS_ENUM: &str = "enum E<T: S<bool>> {\n    End\n    A(E<T.Item>)\n}\nfn main() {}";

/// A choice that passes the type on as it is, or shorter, or that names no parameter, or that a
/// conformance makes for other interface arguments than the use has, closes no cycle, and the
/// enum that holds it stays unmarked.
#[test]
fn choice_of_an_associated_type_that_does_not_grow_should_not_be_reported() {
    let item = "interface I {\n    type Item\n}\n";
    let chain = "interface I {\n    type Item\n}\ninterface Nx {\n    type Next: I\n}\n";
    let list = "enum F: Eq {\n    X(E<W<i64>>)\n}\nfn main() {}";
    let rows = [
        (
            "a choice that shrinks the type",
            format!(
                "{item}class W<T>: I {{\n    type Item = T\n}}\nenum E<T: I> {{\n    End\n    A(T.Item)\n}}\n{list}"
            ),
        ),
        (
            "a choice that names the enum at the depth of its target",
            format!(
                "{item}class W<T>: I {{\n    type Item = E<W<T>>\n}}\nenum E<T: I> {{\n    End\n    A(T.Item)\n}}\n{list}"
            ),
        ),
        (
            // The enum holds `T.Item`, so the choice is read: it names the enum with the
            // parameter of its target, one level shorter than `E<W<T>>` does.
            "a choice that names the enum one level shorter",
            format!(
                "{item}class Z: I {{\n    type Item = i64\n}}\nclass W<T: I>: I {{\n    type Item = E<T>\n}}\nenum E<T: I> {{\n    End\n    A(T.Item)\n}}\nenum F: Eq {{\n    X(E<W<Z>>)\n}}\nfn main() {{}}"
            ),
        ),
        (
            "a choice that names no parameter",
            format!(
                "{item}class W<T>: I {{\n    type Item = i64\n}}\nenum E<T: I> {{\n    End\n    A(T.Item)\n}}\n{list}"
            ),
        ),
        (
            "an associated type of an associated type that shrinks",
            format!(
                "{chain}class X<T>: Nx {{\n    type Next = W<T>\n}}\nclass W<T>: I {{\n    type Item = E<X<T>>\n}}\nenum E<T: Nx> {{\n    End\n    A(T.Next.Item)\n}}\nenum F: Eq {{\n    Y(E<X<i64>>)\n}}\nfn main() {{}}"
            ),
        ),
        // A choice that a conformance to `S<i64>` makes is not one that `T.Item` with `T: S<bool>`
        // can mean, whatever it names.
        (
            "an enum, and a choice made for other interface arguments that grows",
            format!("{S}{OTHER_ARGUMENTS}enum E<T: S<bool>> {{\n    End\n    A(T.Item)\n}}\nfn main() {{}}"),
        ),
        (
            "a class, and a choice made for other interface arguments that grows",
            format!("{S}class W<T>: S<i64> {{\n    type Item = H<W<W<T>>>\n}}\nextend<T> W<T>: S<bool> {{\n    type Item = i64\n}}\nclass H<T: S<bool>> {{\n    v: T.Item?\n}}\nfn main() {{}}"),
        ),
        (
            "an enum that holds the enum of the associated type, and a choice that grows",
            format!("{PARTS}extend<T> W<T>: S<i64> {{\n    type Item = W<W<T>>\n}}\nextend<T> W<T>: S<bool> {{\n    type Item = Z\n}}\n{PARTS_ENUM}"),
        ),
        (
            "the same, and a choice for the other arguments that keeps the depth",
            format!("{PARTS}extend<T> W<T>: S<i64> {{\n    type Item = W<W<T>>\n}}\nextend<T> W<T>: S<bool> {{\n    type Item = W<T>\n}}\n{PARTS_ENUM}"),
        ),
        (
            "an interface with two type parameters",
            "interface S<A, B> {\n    type Item\n}\nclass W<T>: S<i64, bool> {\n    type Item = E<W<W<T>>>\n}\nextend<T> W<T>: S<bool, i64> {\n    type Item = i64\n}\nenum E<T: S<bool, i64>> {\n    End\n    A(T.Item)\n}\nfn main() {}".to_string(),
        ),
        // The associated type is of unbounded growth for `U`, which no value of the program is.
        (
            "an associated type of unbounded growth, and no choice that names a parameter",
            format!("{UNBOUNDED}enum E<T: S> {{\n    End\n    A(T.Next.Item)\n}}\nfn main() {{\n    let e: E<Z> = E.End\n    print(e == e)\n}}"),
        ),
        (
            "an associated type of unbounded growth, and a choice that shrinks the enum",
            format!("{UNBOUNDED}class W<T: S>: S {{\n    type Next = W<T>\n    type Item = E<T>\n}}\nenum E<T: S> {{\n    End\n    A(T.Next.Item)\n}}\nfn main() {{\n    let e: E<W<Z>> = E.End\n    print(e == e)\n}}"),
        ),
        (
            "the same, through a chain of three links",
            format!("{UNBOUNDED}class W<T: S>: S {{\n    type Next = W<T>\n    type Item = E<T>\n}}\nenum E<T: S> {{\n    End\n    A(T.Next.Next.Item)\n}}\nfn main() {{\n    let e: E<W<Z>> = E.End\n    print(e == e)\n}}"),
        ),
        (
            // The growth of `Next` for `S<i64>` is unbounded, and for `S<bool>` it shrinks: the
            // use of `S<bool>` reads the slot of its own arguments.
            "an associated type of unbounded growth for other interface arguments",
            "interface S<A> {\n    type Next: S<A>\n    type Item\n}\nclass Z: S<bool> {\n    type Next = Z\n    type Item = i64\n}\nclass U<T: S<i64>>: S<i64> {\n    type Next = U<U<T.Next>>\n    type Item = i64\n}\nclass W<T: S<bool>>: S<bool> {\n    type Next = T\n    type Item = E<W<W<T>>>\n}\nenum E<T: S<bool>> {\n    End\n    A(T.Next.Item)\n}\nfn main() {\n    let e: E<W<Z>> = E.End\n    print(e == e)\n}".to_string(),
        ),
        (
            "an interface argument list that no conformance has",
            format!("{S}class W<T>: S<i64> {{\n    type Item = E<W<W<T>>>\n}}\nenum E<T: S<bool>> {{\n    End\n    A(T.Item)\n}}\nfn main() {{}}"),
        ),
    ];
    for (row, src) in rows {
        let found: Vec<Code> = diags(&src).iter().map(Diagnostic::code).collect();
        assert!(
            !found.contains(&Code::InstantiationCycle),
            "row `{row}`: {found:?}"
        );
    }
}

/// An interface with two links that lead to the item, a class `Z` that ends them, and a class `U`
/// that no program uses and that chooses `Other` as `U` again: the link `Other` is never `W`.
const TWO_LINKS: &str = "interface S {\n    type Next: S\n    type Other: S\n    type Item\n}\nclass Z: S {\n    type Next = Z\n    type Other = Z\n    type Item = i64\n}\nclass U<T: S>: S {\n    type Next = U<T>\n    type Other = U<T>\n    type Item = i64\n}\n";

/// A class `W` that ends `Other` with `Z` and grows the enum `E` that holds `T.Other.Item`.
const W_ENDS_OTHER: &str =
    "class W<T: S>: S {\n    type Next = W<T>\n    type Other = Z\n    type Item = E<W<W<T>>>\n}\n";

const COMPARED: &str = "fn main() {\n    let e: E<W<Z>> = E.End\n    print(e == e)\n}";

/// A use `T.Other.Item` means a choice of `Item` that a class makes, and `T.Other` is a type that
/// a choice of `Other` names. When no choice of `Other` names the class of a growing choice of
/// `Item`, the use never means that choice, and the beginning of the link makes no cycle.
#[test]
fn choice_of_a_class_that_the_link_cannot_choose_should_not_be_reported() {
    let rows = [
        (
            "an enum holds the associated type of a link",
            format!(
                "{TWO_LINKS}{W_ENDS_OTHER}enum E<T: S> {{\n    End\n    A(T.Other.Item)\n}}\n{COMPARED}"
            ),
        ),
        (
            "a class holds the associated type of a link",
            format!(
                "{TWO_LINKS}{W_ENDS_OTHER}class C<T: S> {{\n    f: T.Other.Item\n}}\nfn main() {{}}"
            )
            .replace("E<W<W<T>>>", "C<W<W<T>>>"),
        ),
        (
            "a chain of two links, one of which is unbounded for the class that is unused",
            format!(
                "{UNBOUNDED_AND_OTHER}{W_ENDS_OTHER}enum E<T: S> {{\n    End\n    A(T.Next.Other.Item)\n}}\n{COMPARED}"
            ),
        ),
        (
            // The class `W` is a choice of the link, but its choice for `S<bool>` is not one that
            // the use for `S<i64>` can mean.
            "a link that can choose the class, and a choice for other interface arguments",
            format!(
                "{LINKED}class W<T>: S<bool> {{\n    type Link = W<T>\n    type Item = E<W<W<T>>>\n}}\nextend<T> W<T>: S<i64> {{\n    type Link = W<T>\n    type Item = T\n}}\nenum E<T: S<i64>> {{\n    End\n    A(T.Link.Item)\n}}\nfn main() {{}}"
            ),
        ),
    ];
    for (row, src) in rows {
        let found: Vec<Code> = diags(&src).iter().map(Diagnostic::code).collect();
        assert!(
            !found.contains(&Code::InstantiationCycle),
            "row `{row}`: {found:?}"
        );
    }
}

/// An interface with a type parameter, an associated type `Link` that leads to the item, and the
/// item.
const LINKED: &str = "interface S<A> {\n    type Link: S<A>\n    type Item\n}\n";

/// `TWO_LINKS` with a `Next` of the class `U` that grows without end, which the issue shows.
const UNBOUNDED_AND_OTHER: &str = "interface S {\n    type Next: S\n    type Other: S\n    type Item\n}\nclass Z: S {\n    type Next = Z\n    type Other = Z\n    type Item = i64\n}\nclass U<T: S>: S {\n    type Next = U<U<T.Next>>\n    type Other = U<T>\n    type Item = i64\n}\n";

/// A use reads at most `MATCHING_CHOICES` heads and choices to find the choices that it can mean.
/// Above that, it feeds all the choices of the associated type, so a class that the link cannot
/// choose gives the error: the check is cautious, and its graph stays in proportion to the
/// program.
#[test]
fn use_of_a_link_above_the_cap_should_feed_every_choice() {
    // `heads` classes `C0`, `C1` and so on are the types that the link `Link` can be, and none of
    // them is `W`, whose choice of `Item` grows: the filter shows that it is not meant.
    let heads = |n: usize| {
        let mut src = String::from(
            "interface S {\n    type Link: S\n    type Item\n}\nclass W<T>: S {\n    type Link = C0<T>\n    type Item = E<W<W<T>>>\n}\nenum E<T: S> {\n    End\n    A(T.Link.Item)\n}\n",
        );
        for i in 0..n {
            let next = (i + 1) % n;
            writeln!(
                src,
                "class C{i}<T>: S {{\n    type Link = C{next}<T>\n    type Item = i64\n}}"
            )
            .expect("writing to a `String` should succeed");
        }
        src + "fn main() {}"
    };
    // The link is always `W`, which makes `n` choices of `Item`, one for each of `n` interface
    // arguments. The class `X` is no head, and its choice of `Item` for the arguments of the use
    // grows: only the hub of all the choices reaches it.
    let choices = |n: usize| {
        let mut src = String::from(
            "interface S<A> {\n    type Link: S<A>\n    type Item\n}\nclass X<T>: S<K0> {\n    type Link = W<T>\n    type Item = E<X<X<T>>>\n}\nenum E<T: S<K0>> {\n    End\n    A(T.Link.Item)\n}\nclass W<T>: S<K0> {\n    type Link = W<T>\n    type Item = T\n}\n",
        );
        for i in 0..n {
            writeln!(src, "class K{i} {{}}").expect("writing to a `String` should succeed");
            if i > 0 {
                writeln!(
                    src,
                    "extend<T> W<T>: S<K{i}> {{\n    type Link = W<T>\n    type Item = T\n}}"
                )
                .expect("writing to a `String` should succeed");
            }
        }
        src + "fn main() {}"
    };
    // A head and a choice each cost one read, and the cap is 64: the rows on each side of it.
    let rows = [
        ("3 heads", heads(3), false),
        ("64 heads", heads(64), false),
        ("65 heads", heads(65), true),
        ("70 heads", heads(70), true),
        ("63 choices and a head", choices(63), false),
        ("64 choices and a head", choices(64), true),
        ("80 choices and a head", choices(80), true),
    ];
    for (row, src, reported) in rows {
        let found: Vec<Code> = diags(&src).iter().map(Diagnostic::code).collect();
        assert_eq!(
            found.contains(&Code::InstantiationCycle),
            reported,
            "row `{row}`: {found:?}"
        );
    }
}

/// A class whose link `Other` can name `W` keeps the error: the graph reads the heads that a link
/// names directly, not the head that each turn of the cycle gives. `W` ends `Other` with `Z`, so
/// the second turn never comes back to `W`, and the error is still reported. This pins the known
/// limit of the head filter.
#[test]
fn choice_of_a_class_that_a_link_can_choose_should_be_reported_even_when_the_next_turn_cannot() {
    let src = format!(
        "{}{W_ENDS_OTHER}enum E<T: S> {{\n    End\n    A(T.Other.Item)\n}}\n{COMPARED}",
        TWO_LINKS.replace("type Other = U<T>", "type Other = W<T>")
    );
    codes(
        &src,
        &[(Code::InstantiationCycle, where_is(&src, "E<W<W<T>>>"))],
    );
}

/// The marks of growing enums use the same graph: an enum that holds `T.Other.Item` is not marked
/// when no choice of `Other` names the growing class, so a query for `Eq` on it reads its
/// payloads, and finds the class with no `Eq` (E0154).
#[test]
fn enum_that_holds_the_choice_of_a_link_that_cannot_grow_should_not_be_marked() {
    let src = format!(
        "{TWO_LINKS}{W_ENDS_OTHER}class Plain {{}}\nenum E<T: S> {{\n    End\n    A(T.Other.Item, Plain)\n}}\nfn need_eq<T: Eq>(x: T) {{}}\nfn f(x: E<W<Z>>) {{\n    need_eq(x)\n}}\nfn main() {{}}"
    );
    codes(&src, &[(Code::NotConform, argument(&src, "need_eq(x)"))]);
}

/// The edges of a use that can mean `n` choices stay in proportion to the program, and so do the
/// reads that find them: above `MATCHING_CHOICES`, the use feeds the hub of all the choices.
#[test]
fn uses_of_a_link_with_many_choices_should_cost_edges_in_proportion_to_the_program() {
    // `n` classes that each choose `Link` as the next class and make a choice of `Item`, so that
    // a use `T.Link.Item` can mean `n` choices, and `n` enums that each hold one such use: with
    // a gate for each match the edges are `n` times `n`. The sizes are ten times the cap and
    // more, so that the fall back is what keeps them in line.
    let edges = |n: usize| {
        let mut src = String::from(
            "interface S {\n    type Link: S\n    type Item\n}\nclass Box<T> {\n    v: T\n}\n",
        );
        for i in 0..n {
            let next = (i + 1) % n;
            writeln!(
                src,
                "class C{i}<T>: S {{\n    type Link = C{next}<T>\n    type Item = Box<T>\n}}\nenum U{i}<T: S> {{\n    End\n    A(T.Link.Item)\n}}"
            )
            .expect("writing to a `String` should succeed");
        }
        src += "fn main() {}";
        cycle::EDGES.set(0);
        cycle::MATCHING_READS.set(0);
        let found: Vec<Code> = diags(&src).iter().map(Diagnostic::code).collect();
        assert!(!found.contains(&Code::InstantiationCycle), "{found:?}");
        (cycle::EDGES.get(), cycle::MATCHING_READS.get())
    };
    let (one, two, three) = (edges(640), edges(1280), edges(1920));
    assert_near_linear(one.0, two.0, three.0);
    // A use reads at most the cap, so the reads, too, stay in proportion to the program.
    assert_near_linear(one.1, two.1, three.1);
}

#[test]
fn conformance_lists_should_accept_generated_and_by_name_conformances() {
    let main = "fn main() {}";
    for (row, src) in [
        (
            "a listed generated Eq or Display with no method",
            format!(
                "{NEEDS}enum Expr: Eq {{\n    Num(i64)\n    Add(Expr, Expr)\n}}\nenum Wrap<T>: Eq {{\n    A(T)\n}}\nclass Node: Display {{}}\n{main}"
            ),
        ),
        (
            "an empty extend block of a generated Eq",
            format!(
                "{NEEDS}enum Expr {{\n    Num(i64)\n    Add(Expr, Expr)\n}}\nextend Expr: Eq {{}}\nfn f(x: Expr) {{\n    need_eq(x)\n}}\n{main}"
            ),
        ),
        (
            "an empty extend block of a generated Display",
            format!("{NEEDS}class Node {{}}\nextend Node: Display {{}}\n{main}"),
        ),
        (
            "written Eq and Ord",
            format!(
                "{NEEDS}class Circle: Eq, Ord {{\n    fn eq(self, other: Circle) -> bool {{ true }}\n    fn less(self, other: Circle) -> bool {{ true }}\n}}\nfn f(x: Circle) {{\n    need_eq(x)\n    need_ord(x)\n}}\n{main}"
            ),
        ),
        (
            "a method named to_string",
            format!(
                "{NEEDS}class Circle {{\n    fn to_string(self) -> string {{ \"c\" }}\n}}\nfn f(x: Circle) {{\n    need_display(x)\n}}\n{main}"
            ),
        ),
        (
            "a method named to_string in a plain extend block",
            format!(
                "{NEEDS}class Circle {{}}\nextend Circle {{\n    fn to_string(self) -> string {{ \"c\" }}\n    fn other(self) -> i64 {{ 1 }}\n}}\nfn f(x: Circle) {{\n    need_display(x)\n}}\n{main}"
            ),
        ),
        (
            "a listed Display met by the method of the body",
            format!(
                "{NEEDS}class Circle: Display {{\n    fn to_string(self) -> string {{ \"c\" }}\n}}\n{main}"
            ),
        ),
        (
            "an interface that inherits Display, met by the method of the body",
            format!(
                "{NEEDS}interface Shape: Display {{}}\nclass Circle: Shape {{\n    fn to_string(self) -> string {{ \"c\" }}\n}}\n{main}"
            ),
        ),
        (
            "a class that lists AnyObject",
            format!("{NEEDS}class C: AnyObject {{}}\n{main}"),
        ),
        (
            "a class of an interface that inherits AnyObject",
            format!(
                "{NEEDS}interface Observer: AnyObject {{}}\nclass Button: Observer {{}}\nfn f(x: Button) {{\n    need_object(x)\n}}\n{main}"
            ),
        ),
    ] {
        clean_row(row, &src);
    }
}

#[test]
fn conformance_lists_should_report_what_is_not_generated() {
    let circle =
        "class Circle: Ord {\n    fn less(self, other: Circle) -> bool { true }\n}\nfn main() {}";
    for (row, src, expected) in [
        (
            "a class that lists Eq with no method",
            "class Node: Eq {}\nfn main() {}",
            vec![(Code::ConformanceBody, (1, 13))],
        ),
        (
            "an enum that lists Ord with no method",
            "enum E: Ord {\n    A\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (1, 9))],
        ),
        (
            "an enum that lists Eq and holds a class",
            "class Node {}\nenum H: Eq {\n    N(Node)\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (2, 9))],
        ),
        // The payload that fails its bound is unknown, so the note names the part that follows.
        (
            "an enum that lists Eq, with a payload that fails its bound and a class",
            "class Node {}\nclass NeedOrd<T: Ord> {}\nenum H: Eq {\n    A(NeedOrd<bool>)\n    B(Node)\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (3, 9)), (Code::NotConform, (4, 15))],
        ),
        (
            "a class that lists Ord with less alone",
            circle,
            vec![(Code::ConformanceBody, (1, 15))],
        ),
        (
            "a block of Ord with no Eq",
            "class Circle {}\nextend Circle: Ord {\n    fn less(self, other: Circle) -> bool { true }\n}\nfn main() {}",
            vec![(Code::NotConform, (2, 16))],
        ),
        (
            "a to_string that returns the wrong type",
            "class C {\n    fn to_string(self) -> i64 { 1 }\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (2, 8))],
        ),
        (
            "an enum that lists AnyObject",
            "enum E: AnyObject {\n    A\n}\nfn main() {}",
            vec![(Code::ClassOnly, (1, 9))],
        ),
        (
            "an enum that lists an interface that inherits AnyObject",
            "interface Observer: AnyObject {}\nenum E: Observer {\n    A\n}\nfn main() {}",
            vec![(Code::ClassOnly, (2, 9))],
        ),
        (
            "a block of an interface that inherits AnyObject for an enum",
            "interface Observer: AnyObject {}\nenum E {\n    A\n}\nextend E: Observer {}\nfn main() {}",
            vec![(Code::ClassOnly, (5, 11))],
        ),
        (
            "a block of Display beside a method named to_string",
            "class Circle {\n    fn to_string(self) -> string { \"c\" }\n}\nextend Circle: Display {\n    fn to_string(self) -> string { \"c\" }\n}\nfn main() {}",
            vec![(Code::ConformsTwice, (4, 16))],
        ),
    ] {
        codes_row(row, src, &expected);
    }
}

#[test]
fn plain_blocks_and_eq_blocks_should_report_only_what_is_wrong() {
    for (row, src, expected) in [
        // A listed `Display` has no method of its own: a method in a plain block meets it, with the
        // signature of the requirement, as it does for a type that lists nothing.
        (
            "a listed Display and a plain block with a to_string of the wrong type",
            "class C: Display {}\nextend C {\n    fn to_string(self) -> i64 { 1 }\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (3, 8))],
        ),
        (
            "a listed Display of an enum and a plain block with a to_string of the wrong type",
            "enum E: Display {\n    A\n}\nextend E {\n    fn to_string(self) -> i64 { 1 }\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (5, 8))],
        ),
        (
            "a listed Display of a generic class and a plain block with a to_string of the wrong type",
            "class Box<T>: Display {\n    v: T\n}\nextend<T> Box<T> {\n    fn to_string(self) -> i64 { 1 }\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (5, 8))],
        ),
        (
            "a Display of an interface that inherits it and a plain block with a to_string of the wrong type",
            "interface Shape: Display {}\nclass C: Shape {}\nextend C {\n    fn to_string(self) -> i64 { 1 }\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (4, 8))],
        ),
        // The fixture `E0152-generated-eq-block` pins the note, which names only a part that fails.
        (
            "a block of Eq with a method of another name, for an enum that gets an Eq",
            "enum E {\n    A\n}\nextend E: Eq {\n    fn helper(self) -> i64 { 1 }\n}\nfn main() {}",
            vec![
                (Code::ConformanceBody, (4, 11)),
                (Code::ConformanceBody, (5, 8)),
            ],
        ),
        // A method of one name twice has the one error, whatever holds the methods.
        (
            "a to_string in the body and in a plain block",
            "class C {\n    fn to_string(self) -> string { \"c\" }\n}\nextend C {\n    fn to_string(self) -> string { \"d\" }\n}\nfn main() {}",
            vec![(Code::DefinedTwice, (5, 8))],
        ),
        (
            "a to_string in two plain blocks",
            "class C {}\nextend C {\n    fn to_string(self) -> string { \"c\" }\n}\nextend C {\n    fn to_string(self) -> string { \"d\" }\n}\nfn main() {}",
            vec![(Code::DefinedTwice, (6, 8))],
        ),
        (
            "a listed Display, a to_string in the body and in a plain block",
            "class C: Display {\n    fn to_string(self) -> string { \"c\" }\n}\nextend C {\n    fn to_string(self) -> string { \"d\" }\n}\nfn main() {}",
            vec![(Code::DefinedTwice, (5, 8))],
        ),
    ] {
        codes_row(row, src, &expected);
    }
}

/// A call of `to_string` through a bound of `Display` checks for a number and for an enum, as
/// `eq` and `less` do (`operators_should_check_through_eq_and_ord`).
#[test]
fn to_string_through_a_bound_should_check_for_every_type() {
    clean(
        "enum Expr {\n    Num(i64)\n}\nfn s<T: Display>(x: T) -> string { x.to_string() }\nfn main() {\n    print(s(1))\n    print(s(Expr.Num(1)))\n}",
    );
}

/// `to_string` is a method of every type but `unit` and `never`: a generated `Display` answers
/// it, for a type parameter with no bound too.
#[test]
fn to_string_should_check_on_a_value_of_every_type() {
    clean(
        "class Node {}\nenum Expr {\n    Num(i64)\n}\nfn any<T>(x: T) -> string { x.to_string() }\nfn main() {\n    let n: i64?? = None\n    print(5.to_string())\n    print(2.5.to_string())\n    print(true.to_string())\n    print('c'.to_string())\n    print(\"s\".to_string())\n    print([1, 2].to_string())\n    print(n.to_string())\n    print(Node().to_string())\n    print(Expr.Num(1).to_string())\n    print(any(1))\n}",
    );
}

/// A `to_string` that takes an argument is an argument-count error, not a call of the
/// generated one, whatever the receiver: a class and an enum with no written `to_string` have the
/// generated one, so the call is no unknown method.
#[test]
fn to_string_with_an_argument_should_report_the_count() {
    let decls = "class C {}\nenum E {\n    A\n}\n";
    for (row, param) in [
        ("a number", "i64"),
        ("a class", "C"),
        ("an enum", "E"),
        ("an optional", "i64?"),
        ("an array", "[i64]"),
        ("a type parameter", "T"),
    ] {
        let generic = if param == "T" { "<T>" } else { "" };
        let src = format!(
            "{decls}fn f{generic}(x: {param}) -> string {{\n    x.to_string(1)\n}}\nfn main() {{}}"
        );
        let at = where_is(&src, "x.to_string(1)");
        codes_row(row, &src, &[(Code::ArgumentCount, at)]);
    }
}

/// A list of type arguments after `to_string` is the error of a method that takes none, whatever
/// the receiver: the generated `Display` takes none either, and a list that names a real type is
/// no better than one that names none. The rows with a written `to_string` are the control: they
/// have the same code at the same place.
#[test]
fn to_string_with_type_arguments_should_report_the_list() {
    let decls = "class C {}\nclass W {}\nextend W {\n    fn to_string(self) -> string { \"w\" }\n}\nclass B {}\nextend B: Display {\n    fn to_string(self) -> string { \"b\" }\n}\nenum E {\n    A\n}\nclass Node {}\nclass Plain<T> {\n    _value: T\n}\nextend<T: Eq> Plain<T> {\n    fn to_string(self) -> string { \"plain\" }\n}\nclass Boxed<T> {\n    _value: T\n}\nextend<T: Eq> Boxed<T>: Display {\n    fn to_string(self) -> string { \"boxed\" }\n}\n";
    for (row, param) in [
        ("a bool", "bool"),
        ("a char", "char"),
        ("a u8", "u8"),
        ("a number", "i64"),
        ("an f64", "f64"),
        ("a string", "string"),
        ("an array", "[i64]"),
        ("an optional", "i64?"),
        ("a class", "C"),
        ("an enum", "E"),
        ("an unbounded type parameter", "T"),
        ("a class with a written to_string", "W"),
        ("a class with a Display block", "B"),
        ("a class whose plain block's bounds fail", "Plain<Node>"),
        (
            "a type parameter whose plain block's bounds fail",
            "Plain<T>",
        ),
        ("a class whose Display block's bounds fail", "Boxed<Node>"),
    ] {
        let generic = if param.ends_with("<T>") || param == "T" {
            "<T>"
        } else {
            ""
        };
        for list in ["<Missing>", "<i64>"] {
            let src = format!(
                "{decls}fn f{generic}(x: {param}) -> string {{\n    x.to_string{list}()\n}}\nfn main() {{}}"
            );
            let at = where_is(&src, list);
            codes_row(
                &format!("{row} with {list}"),
                &src,
                &[(Code::TypeArguments, at)],
            );
        }
    }
}

/// A list of type arguments after `to_string` goes to a written method that has type parameters
/// of its own, even when the bounds of its block fail: the failed bound is reported, as it is for
/// a call with an argument, not "takes no type arguments", which is true for the generated one
/// only. The method does not match `Display`, which is its own error.
#[test]
fn to_string_with_type_arguments_should_report_the_failed_bound_of_a_masked_generic_method() {
    let head = "class Node {}\nclass Own<T> {\n    _value: T\n}\n";
    for (row, block, call) in [
        (
            "a plain block",
            "extend<T: Eq> Own<T> {\n    fn to_string<U>(self) -> string { \"own\" }\n}\n",
            "Own(_value: Node()).to_string<i64>()",
        ),
        (
            "a plain block with two type parameters",
            "extend<T: Eq> Own<T> {\n    fn to_string<U, V>(self) -> string { \"own\" }\n}\n",
            "Own(_value: Node()).to_string<i64>()",
        ),
        (
            "a Display block",
            "extend<T: Eq> Own<T>: Display {\n    fn to_string<U>(self) -> string { \"own\" }\n}\n",
            "Own(_value: Node()).to_string<i64>()",
        ),
    ] {
        let src = format!("{head}{block}fn main() {{\n    print({call})\n}}");
        let at = where_is(&src, call);
        codes_row(
            row,
            &src,
            &[(Code::ConformanceBody, (6, 8)), (Code::NotConform, at)],
        );
    }
}

/// The text of a `to_string` is the one of the type's `Display`: a written method whose bounds
/// fail for the receiver gives way to the generated `Display`, in a plain block and in a block
/// of `Display`.
#[test]
fn to_string_should_fall_back_to_generated_display_when_a_bound_fails() {
    clean(
        "class Node {}\nclass Plain<T> {\n    _value: T\n}\nextend<T: Eq> Plain<T> {\n    fn to_string(self) -> string { \"plain\" }\n}\nclass Box<T> {\n    _value: T\n}\nextend<T: Eq> Box<T>: Display {\n    fn to_string(self) -> string { \"box\" }\n}\nfn main() {\n    print(Plain(_value: Node()).to_string())\n    print(Plain(_value: 1).to_string())\n    print(Box(_value: Node()).to_string())\n    print(Box(_value: 1).to_string())\n}",
    );
}

/// Only a call of `to_string` with no argument has the fallback: another method of a block whose
/// bounds fail keeps the report of the failed bound, and so does `to_string` with an argument.
/// The block is a plain block, or a block of `Display`, whose bounds fail.
#[test]
fn a_method_other_than_to_string_should_keep_the_failed_bound_report() {
    let head = "class Node {}\nclass Plain<T> {\n    _value: T\n}\n";
    for (row, block, call) in [
        (
            "name of a plain block",
            "extend<T: Eq> Plain<T> {\n    fn name(self) -> string { \"plain\" }\n    fn to_string(self) -> string { \"plain\" }\n}\n",
            "name()",
        ),
        (
            "to_string with an argument, of a plain block",
            "extend<T: Eq> Plain<T> {\n    fn name(self) -> string { \"plain\" }\n    fn to_string(self) -> string { \"plain\" }\n}\n",
            "to_string(1)",
        ),
        (
            "to_string with an argument, of a block of Display",
            "extend<T: Eq> Plain<T>: Display {\n    fn to_string(self) -> string { \"plain\" }\n}\n",
            "to_string(1)",
        ),
    ] {
        let src = format!("{head}{block}fn main() {{\n    print(Plain(_value: Node()).{call})\n}}");
        let at = where_is(&src, "Plain(_value: Node())");
        codes_row(row, &src, &[(Code::NotConform, at)]);
    }
}

/// A field named `to_string` is no method, so the call of it is the error of a call of a field:
/// the generated `Display` does not answer it.
#[test]
fn to_string_should_not_answer_a_call_of_a_field_of_that_name() {
    codes(
        "class C {\n    to_string: i64\n}\nfn main() {\n    print(C(to_string: 1).to_string())\n}",
        &[(Code::FieldNotMethod, (5, 27))],
    );
}

/// A receiver that failed but kept its type leaves the call a `string`, and the call adds no
/// error of its own: the part `{g()}` of the receiver has no value, which is the one error.
#[test]
fn to_string_on_a_receiver_that_failed_should_report_only_the_receiver() {
    codes(
        "fn g() {}\nfn f() {\n    print(\"{g()}\".to_string())\n}\nfn main() {}",
        &[(Code::CannotInterpolate, (3, 13))],
    );
}

/// A precision is for an `f64` only, whatever the part: an enum and a type parameter get the
/// error too (the fixtures `E0111-enum` and `E0111-parameter` pin the text, which names the type).
#[test]
fn a_precision_on_a_part_that_is_not_an_f64_should_be_reported() {
    for (decls, param) in [("enum E {\n    A\n}\n", "e: E"), ("", "x: T")] {
        let generic = if param.ends_with('T') { "<T>" } else { "" };
        let part = param.split(':').next().expect("a parameter has a name");
        let src = format!(
            "{decls}fn f{generic}({param}) -> string {{\n    \"{{{part}:.2}}\"\n}}\nfn main() {{}}"
        );
        let line = decls.lines().count() + 2;
        codes(&src, &[(Code::PrecisionNeedsF64, (line, 8))]);
    }
}

/// How `lang_path` finds the language interface `interface` for `ty`, once the checker has
/// recorded every conformance of `src`.
fn path_of(src: &str, ty: &str, interface: &str) -> LangPath {
    // The type is the parameter of a function that follows the program.
    with_probe(src, ("T", ty), interface, |checker, ty, interface| {
        checker.lang_path(ty, interface)
    })
}

/// Checks `src` up to its conformances, with the function `probe<params>(x: ty)` after it, and
/// gives `f` the checker, the type `ty` with the parameters in scope, and the language interface
/// `interface`. A program may be broken: the checker then holds the poison of its broken items.
fn with_probe<R>(
    src: &str,
    (params, ty): (&str, &str),
    interface: &str,
    f: impl FnOnce(&mut Checker<'_>, Type, tir::InterfaceId) -> R,
) -> R {
    let src = format!("{src}fn probe<{params}>(x: {ty}) {{}}\n");
    let lexed = crate::lexer::lex(&src).expect("the source should lex");
    let program = crate::parser::parse(lexed, false).program;
    let prelude = prelude_program("");
    let mut checker = Checker::new(&program, &src);
    let items: Vec<&ast::Item> = program.items.iter().chain(&prelude.items).collect();
    checker.declare_items(&items, program.items.len(), &program.broken);
    let probe = (checker.signatures.iter())
        .find(|signature| signature.name == "probe")
        .expect("the program should declare `probe`");
    let ty = probe.params[0].expect("the row should name a type of the program");
    let (scope, bounds) = (probe.type_params.clone(), probe.bounds.clone());
    let interface = match interface {
        "Display" => checker.lang.display,
        "Eq" => checker.lang.eq,
        "Ord" => checker.lang.ord,
        "AnyObject" => checker.lang.any_object,
        other => panic!("{other} is no language interface of the rows"),
    };
    let (out, _) = checker.with_body(Body::default(), |checker| {
        checker.body.set_type_params(&scope);
        checker.body.set_bounds(0, bounds);
        f(checker, ty, interface)
    });
    out
}

#[test]
fn lang_path_should_choose_the_path_of_each_language_interface() {
    let boxed = "class Node {}\nclass Box<T> {\n    v: T\n}\n";
    let eq = "extend<T: Ord> Box<T>: Eq {\n    fn eq(self, other: Box<T>) -> bool { true }\n}\n";
    let display =
        "extend<T: Eq> Box<T>: Display {\n    fn to_string(self) -> string { \"b\" }\n}\n";
    let rows = [
        // A written conformance whose bounds hold.
        (
            "written Eq",
            format!("{boxed}{eq}"),
            "Box<i64>",
            "Eq",
            LangPath::Written(0),
        ),
        // Its bounds fail: no fallback to the generated rule, which has no `Eq` for a class.
        (
            "written Eq that fails",
            format!("{boxed}{eq}"),
            "Box<bool>",
            "Eq",
            LangPath::None,
        ),
        (
            "class with no Eq",
            boxed.to_string(),
            "Node",
            "Eq",
            LangPath::None,
        ),
        // `Display` falls back to the generated rule when each written record fails.
        (
            "written Display",
            format!("{boxed}{display}"),
            "Box<i64>",
            "Display",
            LangPath::Written(0),
        ),
        (
            "written Display that fails",
            format!("{boxed}{display}"),
            "Box<Node>",
            "Display",
            LangPath::Generated,
        ),
        (
            "class with no Display",
            boxed.to_string(),
            "Node",
            "Display",
            LangPath::Generated,
        ),
        ("number", String::new(), "i64", "Eq", LangPath::Generated),
        (
            "number Ord",
            String::new(),
            "i64",
            "Ord",
            LangPath::Generated,
        ),
        ("bool Ord", String::new(), "bool", "Ord", LangPath::None),
        (
            "enum",
            "enum Dir {\n    North\n}\n".to_string(),
            "Dir",
            "Eq",
            LangPath::Generated,
        ),
        // A broken block may have given the type the conformance.
        (
            "broken block",
            "class C {}\nextend C: {}\n".to_string(),
            "C",
            "Eq",
            LangPath::PoisonOnly,
        ),
        // A conformance with too few methods has its own error, and no code is made from it.
        (
            "record with no methods",
            "class C: Eq {}\n".to_string(),
            "C",
            "Eq",
            LangPath::PoisonOnly,
        ),
    ];
    for (row, src, ty, interface, expected) in rows {
        assert_eq!(path_of(&src, ty, interface), expected, "{row}");
    }
}

#[test]
fn lang_path_should_find_the_to_string_of_a_plain_block_as_the_display() {
    let rows = [
        // A `to_string` in a plain block replaces the generated `Display`, listed or not.
        (
            "listed Display and a plain block with to_string",
            "class C: Display {}\nextend C {\n    fn to_string(self) -> string { \"c\" }\n}\n"
                .to_string(),
            "C",
            "Display",
            LangPath::Written(0),
        ),
        (
            "plain block with to_string",
            "class C {}\nextend C {\n    fn to_string(self) -> string { \"c\" }\n}\n".to_string(),
            "C",
            "Display",
            LangPath::Written(0),
        ),
        (
            "listed Display of an enum and a plain block with to_string",
            "enum E: Display {\n    A\n}\nextend E {\n    fn to_string(self) -> string { \"e\" }\n}\n"
                .to_string(),
            "E",
            "Display",
            LangPath::Written(0),
        ),
        (
            "listed Display with no method",
            "class C: Display {}\n".to_string(),
            "C",
            "Display",
            LangPath::Generated,
        ),
    ];
    for (row, src, ty, interface, expected) in rows {
        assert_eq!(path_of(&src, ty, interface), expected, "{row}");
    }
}

/// The part that makes `ty` fail `interface`, as `failing_leaf` finds it, written as the program
/// does. A type that holds, or one with no part to find, is itself.
fn leaf_of(src: &str, (params, ty): (&str, &str), interface: &str) -> String {
    with_probe(src, (params, ty), interface, |checker, ty, interface| {
        let leaf = checker.failing_leaf(ty, interface, TypeArgs::NONE);
        checker.type_name(leaf)
    })
}

#[test]
fn failing_leaf_should_name_the_part_that_fails_on_its_own() {
    let node = "class Node {}\n";
    let hold = format!("{node}enum H {{\n    N(Node)\n}}\n");
    let pair = "enum Pair<T> {\n    X(T)\n}\nextend<T: Ord> Pair<T>: Eq {\n    fn eq(self, other: Pair<T>) -> bool { true }\n}\n";
    let rows = [
        ("a class", node.to_string(), "Node", "Eq", "Node"),
        ("an enum holding a class", hold.clone(), "H", "Eq", "Node"),
        ("an array of classes", hold, "[H?]", "Eq", "Node"),
        (
            "an enum that meets itself before the class",
            format!("{node}enum A {{\n    X(B)\n    Y(Node)\n}}\nenum B {{\n    Z(A)\n}}\n"),
            "B",
            "Eq",
            "Node",
        ),
        ("a type parameter", String::new(), "[T?]", "Eq", "T"),
        ("a type with no Ord", String::new(), "bool", "Ord", "bool"),
        ("a record whose bound fails", pair.to_string(), "[Pair<bool>]", "Eq", "Pair<bool>"),
        ("a record whose bound holds", pair.to_string(), "[Pair<i64>]", "Eq", "[Pair<i64>]"),
        ("a type that holds", String::new(), "i64", "Eq", "i64"),
        ("a class that holds", node.to_string(), "Node", "AnyObject", "Node"),
        ("an array of classes that hold", node.to_string(), "[Node]", "AnyObject", "[Node]"),
        (
            "an enum that grows",
            "enum Nest<U> {\n    End\n    Next(Nest<[U]>)\n}\n".to_string(),
            "[Nest<i64>]",
            "Eq",
            "[Nest<i64>]",
        ),
        ("a poisoned class", "class C {}\nextend C: {}\n".to_string(), "C", "Eq", "C"),
        (
            "a payload with no choice",
            "interface Plain {\n    type Item\n}\nclass Bare: Plain {}\nenum W<S: Plain> {\n    A(S.Item)\n}\n"
                .to_string(),
            "W<Bare>",
            "Eq",
            "W<Bare>",
        ),
    ];
    for (row, src, ty, interface, leaf) in rows {
        assert_eq!(leaf_of(&src, ("T", ty), interface), leaf, "{row}");
    }
    // A parameter and an associated type fail on their own when their bounds lack the interface.
    let items = "interface Plain {\n    type Item\n}\ninterface Source {\n    type Item: Eq\n}\n";
    let bounded = [
        ("a parameter with the bound", "T: Eq", "[T]", "Eq", "[T]"),
        ("a parameter without the bound", "T: Eq", "T", "Ord", "T"),
        (
            "an item with the bound",
            "S: Source",
            "[S.Item]",
            "Eq",
            "[S.Item]",
        ),
        (
            "an item without the bound",
            "S: Plain",
            "[S.Item]",
            "Eq",
            "S.Item",
        ),
    ];
    for (row, params, ty, interface, leaf) in bounded {
        assert_eq!(leaf_of(items, (params, ty), interface), leaf, "{row}");
    }
}

#[test]
fn failing_leaf_should_not_keep_a_none_that_was_found_with_a_cycle_cut_short() {
    // Read from `Y`, the walk of `X` skips `Y`, which it is inside of, and `X` holds through its
    // other choice: that `None` is true only while `Y` is in progress. Read on its own, `X` goes
    // through `Y` and finds `A`, so the first answer must not be kept for `X`.
    let src = "class A {}\nenum Y {\n    P(X)\n    Q(A)\n}\nenum X {\n    R(Y)\n    S(i64)\n}\nfn other(x: X) {}\n";
    let leaves = with_probe(src, ("T", "Y"), "Eq", |checker, y, eq| {
        let other = (checker.signatures.iter())
            .find(|signature| signature.name == "other")
            .and_then(|signature| signature.params[0])
            .expect("the program should declare `other`");
        [y, other].map(|ty| {
            let leaf = checker.failing_leaf(ty, eq, TypeArgs::NONE);
            checker.type_name(leaf)
        })
    });
    assert_eq!(leaves, ["A", "A"]);
}

/// The step that passes the budget panics, so a query that blows up fails at once.
#[test]
#[should_panic(expected = "conforms should stay within its budget of 3 steps")]
fn conforms_should_panic_at_the_step_that_passes_its_budget() {
    CONFORMS_STEPS.set(0);
    CONFORMS_BUDGET.set(3);
    checked(&chain(5));
}

#[test]
fn a_later_head_should_leave_an_earlier_pending_answer_alone() {
    // `A` waits for `R`, which is open. Then `B` is a head of its own, and commits itself only:
    // `R` fails through `Node`, and `A`, which rested on it, fails with it.
    let decls = "class Node {}\nenum R {\n    P(A)\n    Q(B)\n    W(Node)\n}\nenum A {\n    X(R)\n}\nenum B {\n    Y(i64)\n}";
    let calls = ["need_eq(r)", "need_eq(a)"];
    let src = calling(decls, "r: R, a: A", &calls);
    codes(&src, &failures(&src, &calls));
    // `R`, `A`, `B` and `Node` are each decided once.
    assert_eq!(steps(&src).0, 4);
    // With no `Node` to fail, the same enums hold.
    let decls = "enum R {\n    P(A)\n    Q(B)\n}\nenum A {\n    X(R)\n}\nenum B {\n    Y(i64)\n}";
    assert_eq!(steps(&asking(decls, "eq", "R")), (3, vec![]));
}

#[test]
fn generated_eq_of_an_enum_that_shrinks_through_an_associated_type_should_hold() {
    // The choice `T` of `Wrap<T>` is one level less than its target: no cycle grows.
    let decls = "interface Seq {\n    type Item: Seq\n}\nclass Wrap<T> {\n    v: T\n}\nextend<T: Seq> Wrap<T>: Seq {\n    type Item = T\n}\nclass Stop {}\nextend Stop: Seq {\n    type Item = Stop\n}\nenum Peel<S: Seq> {\n    End\n    Next(Peel<S.Item>)\n}";
    clean(&asking(decls, "eq", "Peel<Wrap<Stop>>"));
}

#[test]
fn generated_eq_of_an_enum_with_a_payload_that_has_no_choice_should_hold() {
    // The class lacks the choice, which is its own error; the payload is poison, not a failure.
    let decls = "interface Plain {\n    type Item\n}\nclass Bare: Plain {}\nenum W<S: Plain> {\n    A(S.Item)\n}";
    let src = asking(decls, "eq", "W<Bare>");
    codes(&src, &[(Code::ConformanceBody, (8, 13))]);
}

#[test]
fn a_top_level_item_should_not_take_the_name_of_a_language_interface() {
    // Each row: the declaration, and the column of its name. A kind of item per row, so that
    // each place that declares a name is checked.
    let kinds: [(&str, &str, usize); 6] = [
        ("class", "class NAME {}", 7),
        ("enum", "enum NAME {\n    A\n}", 6),
        ("interface", "interface NAME {}", 11),
        ("function", "fn NAME() {}", 4),
        ("constant", "const NAME = 1", 7),
        ("type parameter", "fn g<NAME>() {}", 6),
    ];
    for name in ["Display", "Eq", "Ord", "AnyObject"] {
        for (kind, form, column) in kinds {
            let src = format!("{}\nfn main() {{}}", form.replace("NAME", name));
            codes_row(
                &format!("{kind} {name}"),
                &src,
                &[(Code::BuiltInName, (1, column))],
            );
        }
    }
    // The interface a user item meant to replace stays the language's: `Ord` still needs `eq`.
    let src = "interface Eq {}\nclass C: Ord {\n    fn less(self, other: Self) -> bool { true }\n}\nfn main() {}";
    codes(
        src,
        &[
            (Code::BuiltInName, (1, 11)),
            (Code::ConformanceBody, (2, 10)),
        ],
    );
}

#[test]
fn the_uses_of_an_item_that_took_the_name_of_a_language_interface_should_add_no_error() {
    // As for `class Some`: the item takes the name, so its uses are the item's, and the one
    // error stands at its name. Each row: the program, and the column of the name.
    let rows: [(&str, &str, usize); 5] = [
        (
            "a class as a type and a constructor",
            "class NAME {\n    w: i64\n}\nfn width(d: NAME) -> i64 { d.w }\nfn main() {\n    print(width(NAME(w: 1)))\n}",
            7,
        ),
        (
            "an enum as a type and a variant",
            "enum NAME {\n    A\n}\nfn f(e: NAME) {}\nfn main() {\n    f(NAME.A)\n}",
            6,
        ),
        (
            "a function called",
            "fn NAME() -> i64 { 1 }\nfn main() {\n    print(NAME())\n}",
            4,
        ),
        (
            "a constant read",
            "const NAME = 1\nfn main() {\n    print(NAME)\n}",
            7,
        ),
        (
            "a class as the target of an extend block",
            "class NAME {\n    w: i64\n}\nextend NAME {\n    fn get(self) -> i64 { self.w }\n}\nfn main() {\n    print(NAME(w: 1).get())\n}",
            7,
        ),
    ];
    for name in ["Display", "Eq", "Ord", "AnyObject"] {
        for (kind, form, column) in rows {
            codes_row(
                &format!("{kind} {name}"),
                &form.replace("NAME", name),
                &[(Code::BuiltInName, (1, column))],
            );
        }
    }
}

#[test]
fn a_binding_member_or_variant_should_still_take_the_name_of_a_language_interface() {
    // ADR 0015 lets a local, a parameter, a method and a field take a prelude name.
    let src = "class C {\n    Eq: i64\n    fn Ord(self) {}\n}\nenum E {\n    Display\n    AnyObject\n}\nfn f(Display: i64) {\n    let AnyObject = Display\n    print(AnyObject)\n}\nfn main() {}";
    clean(src);
}

#[test]
fn a_payload_that_fails_its_bound_should_be_unknown_to_every_query_in_any_order() {
    // Each row: the declarations that ask for `Eq` of an enum, in two orders; the enum of the
    // payload `NeedOrd<bool>`, which fails its bound, is the only mistake. A query that reads
    // the payload before its check runs would fail on the class `NeedOrd` and add errors.
    let head =
        "class NeedEq<T: Eq> {}\nclass NeedOrd<T: Ord> {}\nenum Wrap<T: Eq> {\n    W(T)\n}\n";
    let rows: [(&str, [&str; 2]); 5] = [
        (
            "a signature asks",
            [
                "fn user(x: NeedEq<E>) {}\nenum E: Eq {\n    A(NeedOrd<bool>)\n}",
                "enum E: Eq {\n    A(NeedOrd<bool>)\n}\nfn user(x: NeedEq<E>) {}",
            ],
        ),
        (
            "a payload of another enum asks",
            [
                "enum E1 {\n    A(Wrap<E2>)\n}\nenum E2 {\n    B(NeedOrd<bool>)\n}",
                "enum E2 {\n    B(NeedOrd<bool>)\n}\nenum E1 {\n    A(Wrap<E2>)\n}",
            ],
        ),
        (
            "enums that ask each other, the mistake in the second payload",
            [
                "enum E1 {\n    A(Wrap<E2>)\n}\nenum E2 {\n    B(Wrap<E1>)\n    C(NeedOrd<bool>)\n}",
                "enum E2 {\n    B(Wrap<E1>)\n    C(NeedOrd<bool>)\n}\nenum E1 {\n    A(Wrap<E2>)\n}",
            ],
        ),
        (
            "a field asks",
            [
                "class K {\n    f: NeedEq<E>\n}\nenum E {\n    A(NeedOrd<bool>)\n}",
                "enum E {\n    A(NeedOrd<bool>)\n}\nclass K {\n    f: NeedEq<E>\n}",
            ],
        ),
        (
            "an interface requirement asks",
            [
                "interface I {\n    fn m(self, x: NeedEq<E>)\n}\nenum E {\n    A(NeedOrd<bool>)\n}",
                "enum E {\n    A(NeedOrd<bool>)\n}\ninterface I {\n    fn m(self, x: NeedEq<E>)\n}",
            ],
        ),
    ];
    for (name, orders) in rows {
        for (i, decls) in orders.iter().enumerate() {
            let src = format!("{head}{decls}\nfn main() {{}}");
            let at = src.find("NeedOrd<bool>").expect("the row has the payload") + "NeedOrd<".len();
            let row = format!("{name}, order {i}");
            codes_row(&row, &src, &[(Code::NotConform, line_col(&src, at))]);
        }
    }
}

#[test]
fn growing_marks_should_search_each_component_once_for_a_shrinking_ring() {
    // `n` enums that each hold the next, and the last the first, with the argument `S.Item`,
    // which is one level less than `S`: the ring has a negative weight and no positive cycle.
    // One search of the ring reads its edges a few times; a search for each of the `n`
    // enums would read them `n` times each.
    let ring = |n: usize| {
        let mut src = String::from(
            "interface Seq {\n    type Item: Seq\n}\nclass Wrap<T> {\n    v: T\n}\nextend<T: Seq> Wrap<T>: Seq {\n    type Item = T\n}\nclass Stop {}\nextend Stop: Seq {\n    type Item = Stop\n}\n",
        );
        for i in 0..n {
            let next = (i + 1) % n;
            writeln!(
                src,
                "enum E{i}<S: Seq> {{\n    End\n    Next(E{next}<S.Item>)\n}}"
            )
            .expect("writing to a `String` should succeed");
        }
        src += "fn main() {}";
        cycle::BELLMAN_FORD_STEPS.set(0);
        clean(&src);
        cycle::BELLMAN_FORD_STEPS.get()
    };
    // 20, 40 and 60 enums: ten times the smallest size that a quadratic search shows.
    assert_near_linear(ring(20), ring(40), ring(60));
}

#[test]
fn growing_marks_should_read_only_the_declarations_that_can_grow() {
    // `n` generic enums that name a generic class and no other declaration, and `rest` besides.
    // The graph of the marks holds the edges of the declarations on a cycle that can grow: none
    // for the `n`, whatever their number.
    let edges = |n: usize, rest: &str| {
        let mut src = String::from("class Wrap<T> {\n    v: T\n}\n");
        for i in 0..n {
            writeln!(src, "enum E{i}<T> {{\n    A(Wrap<T>)\n    B([Wrap<T>])\n}}")
                .expect("writing to a `String` should succeed");
        }
        src += rest;
        src += "fn main() {}";
        cycle::MARK_EDGES.set(0);
        // A cycle that grows is E0140, which the marks do not need to see.
        diags(&src);
        cycle::MARK_EDGES.get()
    };
    // Cycles that pass each parameter on as it is, or that have no parameter, cannot grow.
    let steady = [
        "enum List<T> {\n    Nil\n    Cons(T, List<T>)\n}\n",
        "enum P<A, B> {\n    X(Q<B, A>)\n}\nenum Q<A, B> {\n    Y([P<A, B>])\n}\n",
        "class Node {\n    next: Node?\n}\nenum Tree {\n    Leaf\n    Fork(Tree, Tree)\n}\n",
        // The argument `[T]` is outside the cycle: `Wrap` does not name `List`.
        "enum List<T> {\n    Nil\n    Cons(Wrap<[T]>, List<T>)\n}\n",
        // The argument `[T]` comes from outside the cycle: `List` does not name `Outer`.
        "enum Outer<T> {\n    A(List<[T]>)\n}\nenum List<T> {\n    Nil\n    Cons(T, List<T>)\n}\n",
    ];
    for rest in steady {
        clean(&format!(
            "class Wrap<T> {{\n    v: T\n}}\n{rest}fn main() {{}}"
        ));
        assert_eq!(edges(0, rest), 0, "{rest}");
        assert_eq!(edges(60, rest), 0, "60 more enums: {rest}");
    }
    // A cycle with an argument that is not a parameter alone can grow, and gives its edges.
    let growable = [
        "enum G<T> {\n    End\n    A(G<[T]>)\n}\n",
        "enum P<T> {\n    End\n    X(Q<T>)\n}\nenum Q<T> {\n    Y(P<Wrap<T>>)\n}\n",
        "class K<T> {\n    next: List<T>?\n}\nenum List<T> {\n    Nil\n    Cons(K<[T]>)\n}\n",
    ];
    for rest in growable {
        let alone = edges(0, rest);
        assert!(alone > 0, "the cycle gives edges: {rest}");
        assert_eq!(edges(60, rest), alone, "60 more enums, same edges: {rest}");
    }
}

/// A class with a written `Eq` and `Ord`, which the operator rows compare.
const CIRCLE: &str = "class Circle: Eq, Ord {\n    r: i64\n    fn eq(self, other: Self) -> bool { self.r == other.r }\n    fn less(self, other: Self) -> bool { self.r < other.r }\n}\n";

#[test]
fn operators_should_check_through_eq_and_ord() {
    with_prelude();
    // Every row uses each value it computes, since a bare expression statement is W0004.
    let src = format!(
        "{CIRCLE}enum Pick {{\n    One(Circle)\n    Two\n}}\nenum Expr: Eq {{\n    Num(i64)\n    Add(Expr, Expr)\n}}\n\
         fn largest<T: Ord>(xs: [T]) -> T? {{\n    let mut best: T? = None\n    for x in xs {{\n        if let b = best {{\n            if x > b {{\n                best = x\n            }}\n        }} else {{\n            best = x\n        }}\n    }}\n    best\n}}\n\
         fn same<T: Eq>(a: T, b: T) -> bool {{\n    a == b && !(a != b)\n}}\n\
         fn same_items<T: Eq>(a: [T], b: [T]) -> bool {{\n    a == b\n}}\n\
         fn via_eq<T: Eq>(a: T, b: T) -> bool {{\n    a.eq(b)\n}}\n\
         fn via_less<T: Ord>(a: T, b: T) -> bool {{\n    a.less(b)\n}}\n\
         fn none_test<T>(x: T?) -> bool {{\n    x == None\n}}\n\
         fn main() {{\n    let c1 = Circle(r: 1)\n    let c2 = Circle(r: 2)\n    let c3 = Circle(r: 3)\n\
         \x20   let x = 1.5\n    let y = 2.5\n    let e1 = Expr.Num(1)\n    let e2 = Expr.Num(2)\n\
         \x20   let rows = [c1 == c2, c1 < c2, c1 >= c2, c1 < c2 < c3, [c1] == [c2], Pick.One(c1) == Pick.One(c2), 5.eq(5), x.less(y), [c1].contains(c2), e1 == e2]\n\
         \x20   let best = largest([c1, c2])\n\
         \x20   print(rows.len())\n    print(none_test(best))\n    print(same(c1, c2) || same_items([c1], [c2]) || via_eq(c1, c2) || via_less(c1, c2))\n}}"
    );
    clean(&src);
}

#[test]
fn operators_should_lower_to_the_spec_formulas() {
    let decls = format!(
        "{CIRCLE}enum Expr: Eq {{\n    Num(i64)\n    Add(Expr, Expr)\n}}\nfn f() -> Circle {{ Circle(r: 1) }}\nfn g() -> Circle {{ Circle(r: 2) }}\nfn main() {{\n"
    );
    // Each row: the statement in `main`, and what its `tir` holds.
    let rows: [(&str, &str, &[&str]); 10] = [
        (
            "> swaps the operands",
            "let c1 = Circle(r: 1)\n    let c2 = Circle(r: 2)\n    let r = c1 > c2",
            &["(call Circle.less c2#1 c1#0)"],
        ),
        (
            "<= swaps and negates",
            "let c1 = Circle(r: 1)\n    let c2 = Circle(r: 2)\n    let r = c1 <= c2",
            &["(! (call Circle.less c2#1 c1#0))"],
        ),
        (
            ">= negates",
            "let c1 = Circle(r: 1)\n    let c2 = Circle(r: 2)\n    let r = c1 >= c2",
            &["(! (call Circle.less c1#0 c2#1))"],
        ),
        (
            "!= negates",
            "let c1 = Circle(r: 1)\n    let c2 = Circle(r: 2)\n    let r = c1 != c2",
            &["(! (call Circle.eq c1#0 c2#1))"],
        ),
        (
            "a swapped pair of calls runs the left first",
            "let r = f() > g()",
            &["(let #", "(call f)", "(call g)", "(call Circle.less #"],
        ),
        (
            "a swapped pair with one local binds both",
            "let c1 = Circle(r: 1)\n    let r = c1 > g()",
            &["(let #", "(call g)", "(call Circle.less #"],
        ),
        (
            "a class beside its optional is the generated operator",
            "let c1 = Circle(r: 1)\n    let maybe: Circle? = c1\n    let r = c1 == maybe",
            &["(== (wrap c1#0) maybe#1)"],
        ),
        (
            "a generated eq is the operator",
            "let r = 5.eq(5)",
            &["(== 5 5)"],
        ),
        (
            "a listed Eq on an enum is the operator",
            "let e1 = Expr.Num(1)\n    let e2 = Expr.Num(2)\n    let r = e1 == e2",
            &["(== e1#0 e2#1)"],
        ),
        (
            "a chain runs each operand once and skips the rest",
            "let r = f() < g() < f()",
            &["(&& (call Circle.less #"],
        ),
    ];
    for (row, body, parts) in rows {
        let src = format!("{decls}    {body}\n    print(r)\n}}");
        let printed = lowered(&src);
        for part in parts {
            assert!(
                printed.contains(part),
                "row `{row}`: `{part}` in\n{printed}"
            );
        }
    }
}

#[test]
fn operators_should_report_a_missing_bound() {
    with_prelude();
    let decls = "class Node {}\nclass Box<T> {\n    v: T\n}\nenum Wrap<T> {\n    One(T)\n}\n";
    // Each row: the function after the declarations, the text where the error stands, and the
    // code. The help text of each is pinned in the fixtures `E0139-*`. A row with operands of two
    // types is the mistake of the operands: see
    // `comparisons_should_report_operands_of_two_types_before_a_missing_operator`.
    let rows = [
        (
            "order on a bare parameter",
            "fn f<T>(a: T, b: T) -> bool { a < b }",
            "a < b",
            Code::ParamLacks,
        ),
        (
            "equality on a bare parameter",
            "fn f<T>(a: T, b: T) -> bool { a == b }",
            "a == b",
            Code::ParamLacks,
        ),
        (
            "equality of two parameters",
            "fn f<T, U>(a: T, b: U) -> bool { a == b }",
            "a == b",
            Code::CannotCompare,
        ),
        (
            "order of two parameters",
            "fn f<T, U>(a: T, b: U) -> bool { a < b < a }",
            "b < a",
            Code::Mismatch,
        ),
        (
            "contains of another parameter",
            "fn f<T, U>(a: [T], b: U) -> bool { a.contains(b) }",
            "b)",
            Code::Mismatch,
        ),
        (
            "equality of arrays of a parameter",
            "fn f<T>(a: [T], b: [T]) -> bool { a == b }",
            "a == b",
            Code::ParamLacks,
        ),
        (
            "contains on an array of a parameter",
            "fn f<T>(a: [T], x: T) -> bool { a.contains(x) }",
            "a.contains(x)",
            Code::NotConform,
        ),
        (
            "equality of an enum of a parameter",
            "fn f<T>(a: Wrap<T>, b: Wrap<T>) -> bool { a == b }",
            "a == b",
            Code::ParamLacks,
        ),
        (
            "order beside Display",
            "fn f<T: Display>(a: T, b: T) -> bool { a < b }",
            "a < b",
            Code::ParamLacks,
        ),
        (
            "order beside Eq",
            "fn f<T: Eq>(a: T, b: T) -> bool { a < b }",
            "a < b",
            Code::ParamLacks,
        ),
    ];
    for (row, function, at, code) in rows {
        let src = format!("{decls}{function}\nfn main() {{}}");
        let start = src.find(at).expect("the row should hold its operator");
        codes_row(row, &src, &[(code, line_col(&src, start))]);
    }
    // An argument that does not conform is reported at the argument, not inside the body.
    let src = format!(
        "{decls}fn largest<T: Ord>(xs: [T]) -> T? {{\n    None\n}}\nfn main() {{\n    let n1 = Node()\n    let best = largest([n1])\n    print(best == None)\n}}"
    );
    let start = src.find("[n1]").expect("the call should have its argument");
    codes(&src, &[(Code::NotConform, line_col(&src, start))]);
}

/// The position of the first `needle` in `src`.
fn where_is(src: &str, needle: &str) -> (usize, usize) {
    line_col(
        src,
        src.find(needle).expect("the program should hold the text"),
    )
}

/// Each way that a program spells a call through a language interface closes a cycle that is
/// reported at the spelling: an operator, `contains`, `print`, an interpolation (at its part),
/// or the generated `==` or `to_string` of an enum (at the payload type). The note of a call
/// through a bound stands at the spelling too. The text of each label is pinned by the
/// `E0140-*` fixtures.
#[test]
fn cycle_through_a_language_interface_should_be_reported_at_the_spelling_of_the_call() {
    with_prelude();
    // Each row: the program, where the label stands, and where the note stands, if there is one.
    let rows = [
        (
            "a written `==` on an array of a class",
            "class W<T> {\n    v: T\n}\nextend<T: Eq> W<T>: Eq {\n    fn eq(self, other: Self) -> bool {\n        [W(v: [self.v])] == [W(v: [other.v])]\n    }\n}\nfn main() {\n    print(W(v: 1) == W(v: 1))\n}\n",
            "[W(v: [self.v])] == [W(v: [other.v])]",
            None,
        ),
        (
            "a written `!=`",
            "class W<T> {\n    v: T\n}\nextend<T: Eq> W<T>: Eq {\n    fn eq(self, other: Self) -> bool {\n        [W(v: [self.v])] != [W(v: [other.v])]\n    }\n}\nfn main() {\n    print(W(v: 1) == W(v: 1))\n}\n",
            "[W(v: [self.v])] != [W(v: [other.v])]",
            None,
        ),
        (
            "`contains`",
            "class W<T> {\n    v: T\n}\nextend<T: Eq> W<T>: Eq {\n    fn eq(self, other: Self) -> bool {\n        [W(v: [self.v])].contains(W(v: [other.v]))\n    }\n}\nfn main() {\n    print(W(v: 1) == W(v: 1))\n}\n",
            "[W(v: [self.v])].contains(W(v: [other.v]))",
            None,
        ),
        (
            "`print`",
            "class W<T> {\n    v: T\n}\nextend<T: Display> W<T>: Display {\n    fn to_string(self) -> string {\n        print(W(v: [self.v]))\n        \"w\"\n    }\n}\nfn main() {\n    print(W(v: 1))\n}\n",
            "print(W(v: [self.v]))",
            None,
        ),
        (
            "`eprint`",
            "class W<T> {\n    v: T\n}\nextend<T: Display> W<T>: Display {\n    fn to_string(self) -> string {\n        eprint(W(v: [self.v]))\n        \"w\"\n    }\n}\nfn main() {\n    print(W(v: 1))\n}\n",
            "eprint(W(v: [self.v]))",
            None,
        ),
        (
            "an interpolation",
            "class W<T> {\n    v: T\n}\nextend<T: Display> W<T>: Display {\n    fn to_string(self) -> string {\n        \"{W(v: [self.v])}\"\n    }\n}\nfn main() {\n    print(W(v: 1))\n}\n",
            "W(v: [self.v])",
            None,
        ),
        (
            "the generated `==` of an enum",
            "class W<T> {\n    v: T\n}\nenum E<T> {\n    Has(W<T>)\n}\nextend<T: Eq> W<T>: Eq {\n    fn eq(self, other: Self) -> bool {\n        E.Has(W(v: [self.v])) == E.Has(W(v: [other.v]))\n    }\n}\nfn main() {\n    print(W(v: 1) == W(v: 1))\n}\n",
            "W<T>)",
            Some("E.Has(W(v: [self.v])) == E.Has(W(v: [other.v]))"),
        ),
        (
            "the generated `to_string` of an enum",
            "class W<T> {\n    v: T\n}\nenum E<T> {\n    Has(W<T>)\n}\nextend<T: Display> W<T>: Display {\n    fn to_string(self) -> string {\n        \"{E.Has(W(v: [self.v]))}\"\n    }\n}\nfn main() {\n    print(W(v: 1))\n}\n",
            "W<T>)",
            Some("E.Has(W(v: [self.v]))"),
        ),
        (
            "`==` in a function over a bound",
            "class W<T> {\n    v: T\n}\nfn same<T: Eq>(x: T) -> bool {\n    x == x\n}\nextend<T: Eq> W<T>: Eq {\n    fn eq(self, other: Self) -> bool {\n        same(W(v: [self.v]))\n    }\n}\nfn main() {\n    print(W(v: 1) == W(v: 1))\n}\n",
            "same(W(v: [self.v]))",
            Some("x == x"),
        ),
        (
            "`<` in a function over a bound",
            "class W<T> {\n    v: T\n}\nclass V<T> {\n    v: T\n}\nfn lt<T: Ord>(x: T, y: T) -> bool {\n    x < y\n}\nextend<T: Ord> V<T>: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\nextend<T: Ord> V<T>: Ord {\n    fn less(self, other: Self) -> bool { true }\n}\nextend<T: Ord> W<T>: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\nextend<T: Ord> W<T>: Ord {\n    fn less(self, other: Self) -> bool {\n        lt(W(v: V(v: self.v)), W(v: V(v: other.v)))\n    }\n}\nfn main() {\n    print(W(v: 1) < W(v: 1))\n}\n",
            "lt(W(v: V(v: self.v)), W(v: V(v: other.v)))",
            Some("x < y"),
        ),
        (
            "`<=` in a function over a bound",
            "class W<T> {\n    v: T\n}\nclass V<T> {\n    v: T\n}\nfn lt<T: Ord>(x: T, y: T) -> bool {\n    x <= y\n}\nextend<T: Ord> V<T>: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\nextend<T: Ord> V<T>: Ord {\n    fn less(self, other: Self) -> bool { true }\n}\nextend<T: Ord> W<T>: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\nextend<T: Ord> W<T>: Ord {\n    fn less(self, other: Self) -> bool {\n        lt(W(v: V(v: self.v)), W(v: V(v: other.v)))\n    }\n}\nfn main() {\n    print(W(v: 1) < W(v: 1))\n}\n",
            "lt(W(v: V(v: self.v)), W(v: V(v: other.v)))",
            Some("x <= y"),
        ),
        (
            "`>` in a function over a bound",
            "class W<T> {\n    v: T\n}\nclass V<T> {\n    v: T\n}\nfn lt<T: Ord>(x: T, y: T) -> bool {\n    x > y\n}\nextend<T: Ord> V<T>: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\nextend<T: Ord> V<T>: Ord {\n    fn less(self, other: Self) -> bool { true }\n}\nextend<T: Ord> W<T>: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\nextend<T: Ord> W<T>: Ord {\n    fn less(self, other: Self) -> bool {\n        lt(W(v: V(v: self.v)), W(v: V(v: other.v)))\n    }\n}\nfn main() {\n    print(W(v: 1) < W(v: 1))\n}\n",
            "lt(W(v: V(v: self.v)), W(v: V(v: other.v)))",
            Some("x > y"),
        ),
        (
            "`>=` in a function over a bound",
            "class W<T> {\n    v: T\n}\nclass V<T> {\n    v: T\n}\nfn lt<T: Ord>(x: T, y: T) -> bool {\n    x >= y\n}\nextend<T: Ord> V<T>: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\nextend<T: Ord> V<T>: Ord {\n    fn less(self, other: Self) -> bool { true }\n}\nextend<T: Ord> W<T>: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\nextend<T: Ord> W<T>: Ord {\n    fn less(self, other: Self) -> bool {\n        lt(W(v: V(v: self.v)), W(v: V(v: other.v)))\n    }\n}\nfn main() {\n    print(W(v: 1) < W(v: 1))\n}\n",
            "lt(W(v: V(v: self.v)), W(v: V(v: other.v)))",
            Some("x >= y"),
        ),
        (
            "an interpolation in a function over a bound",
            "class W<T> {\n    v: T\n}\nfn show<T: Display>(x: T) -> string {\n    \"{x}\"\n}\nextend<T: Display> W<T>: Display {\n    fn to_string(self) -> string {\n        show(W(v: [self.v]))\n    }\n}\nfn main() {\n    print(W(v: 1))\n}\n",
            "show(W(v: [self.v]))",
            Some("x}"),
        ),
        (
            "the generated `==` of an enum declared after its use",
            "class W<T> {\n    v: T\n}\nextend<T: Eq> W<T>: Eq {\n    fn eq(self, other: Self) -> bool {\n        E.Has(W(v: [self.v])) == E.Has(W(v: [other.v]))\n    }\n}\nenum E<T> {\n    Has(W<T>)\n}\nfn main() {\n    print(W(v: 1) == W(v: 1))\n}\n",
            "E.Has(W(v: [self.v])) == E.Has(W(v: [other.v]))",
            Some("W<T>)"),
        ),
    ];
    for (row, src, label, note) in rows {
        let notes: Vec<_> = note.into_iter().map(|note| where_is(src, note)).collect();
        let expected = [(Code::InstantiationCycle, where_is(src, label), notes)];
        assert_eq!(found(src), expected, "row `{row}`");
    }
}

/// A program with the class `W<T>`, a conformance of `W<T>` to `interface` (`Eq` or `Display`)
/// whose method has the body `body`, and then the code `after`.
fn conformance_with(interface: &str, after: &str, body: &str) -> String {
    let (method, main) = match interface {
        "Eq" => ("fn eq(self, other: Self) -> bool", "W(v: 1) == W(v: 1)"),
        _ => ("fn to_string(self) -> string", "W(v: 1)"),
    };
    format!(
        "class W<T> {{\n    v: T\n}}\nextend<T: {interface}> W<T>: {interface} {{\n    {method} {{\n        {body}\n    }}\n}}\n{after}fn main() {{\n    print({main})\n}}\n"
    )
}

/// A program with the classes `W<T>` and `V<T>`, which have a written `Eq` and `Ord`; the `less`
/// of `W<T>` has the body `body`.
fn ordered_with(body: &str) -> String {
    let both = |class: &str, less: &str| {
        format!(
            "extend<T: Ord> {class}<T>: Eq {{\n    fn eq(self, other: Self) -> bool {{ true }}\n}}\nextend<T: Ord> {class}<T>: Ord {{\n    fn less(self, other: Self) -> bool {{\n        {less}\n    }}\n}}\n"
        )
    };
    let classes = "class W<T> {\n    v: T\n}\nclass V<T> {\n    v: T\n}\n";
    let (v, w) = (both("V", "true"), both("W", body));
    format!("{classes}{v}{w}fn main() {{\n    print(W(v: 1) < W(v: 1))\n}}\n")
}

/// A method call that the checker lowers to the node of an operator is reported at the whole
/// call. So is a written operator that the checker lowers to a call, or to an edge into the
/// generated node of an enum that its use precedes. The text of each label is pinned by the
/// `E0140-*` fixtures.
#[test]
fn cycle_through_a_lowered_call_should_be_reported_at_the_spelling_of_the_call() {
    with_prelude();
    let (a, b) = ("W(v: [self.v])", "W(v: [other.v])");
    let (wa, wb) = ("W(v: V(v: self.v))", "W(v: V(v: other.v))");
    let (e1, e2) = (format!("E.Has({a})"), format!("E.Has({b})"));
    let after = "enum E<T> {\n    Has(W<T>)\n}\n";
    // Each row: the program, where the label stands, and where the note stands, if there is one.
    let rows = [
        (
            "`.to_string()` on an array",
            conformance_with("Display", "", &format!("[{a}].to_string()")),
            format!("[{a}].to_string()"),
            None,
        ),
        (
            "`.to_string()` on an optional",
            conformance_with(
                "Display",
                "",
                &format!("let x: W<[T]>? = {a}\n        x.to_string()"),
            ),
            "x.to_string()".to_string(),
            None,
        ),
        (
            "`.to_string()` on an enum declared after its use",
            conformance_with("Display", after, &format!("{e1}.to_string()")),
            format!("{e1}.to_string()"),
            Some("W<T>)"),
        ),
        (
            "`.eq()` on an array",
            conformance_with("Eq", "", &format!("[{a}].eq([{b}])")),
            format!("[{a}].eq([{b}])"),
            None,
        ),
        (
            "`.eq()` on an optional",
            conformance_with("Eq", "", &format!("let x: W<[T]>? = {a}\n        x.eq(x)")),
            "x.eq(x)".to_string(),
            None,
        ),
        (
            "`.eq()` on an enum declared after its use",
            conformance_with("Eq", after, &format!("{e1}.eq({e2})")),
            format!("{e1}.eq({e2})"),
            Some("W<T>)"),
        ),
        (
            "`==` on a class with a written `eq`",
            conformance_with("Eq", "", &format!("{a} == {b}")),
            format!("{a} == {b}"),
            None,
        ),
        (
            "`!=` on a class with a written `eq`",
            conformance_with("Eq", "", &format!("{a} != {b}")),
            format!("{a} != {b}"),
            None,
        ),
        (
            "`<` on a class with a written `less`",
            ordered_with(&format!("{wa} < {wb}")),
            format!("{wa} < {wb}"),
            None,
        ),
        (
            "a chain on a class with a written `less`",
            ordered_with(&format!("{wa} < {wb} <= {wa}")),
            format!("{wa} < {wb} <= {wa}"),
            None,
        ),
        (
            "`print` of an enum declared after its use",
            conformance_with("Display", after, &format!("print({e1})\n        \"\"")),
            format!("print({e1})"),
            Some("W<T>)"),
        ),
        // The prelude's `contains` calls `==` through the `Eq` hub, which reaches `W<T>.eq`
        // directly: the shorter cycle passes no generated node, so no note marks the payload.
        (
            "`contains` on an enum declared after its use",
            conformance_with("Eq", after, &format!("[{e1}].contains({e2})")),
            format!("[{e1}].contains({e2})"),
            None,
        ),
    ];
    for (row, src, label, note) in &rows {
        let notes: Vec<_> = note.iter().map(|note| where_is(src, note)).collect();
        let (line, col) = where_is(src, label);
        // The text of the label pins its end: a label over the receiver alone would differ.
        diagnostic_row(
            row,
            src,
            Code::InstantiationCycle,
            (line, col, label),
            &notes,
        );
    }
}

#[test]
fn implicit_eq_call_should_close_an_instantiation_cycle() {
    let w = "class W<T> {\n    x: T\n}\n";
    let main = "fn main() {\n    print(W(x: 1) == W(x: 2))\n}";
    // Each row: the program, and the E0140 it gets, if any, at the text it stands at.
    let rows = [
        (
            "an operand of a class that grows",
            format!(
                "{w}extend<T: Eq> W<T>: Eq {{\n    fn eq(self, other: Self) -> bool {{\n        W(x: [self.x]) == W(x: [other.x])\n    }}\n}}\n{main}"
            ),
            Some("W(x: [self.x]) == W(x: [other.x])"),
        ),
        (
            "an operand of an enum, reached through its generated Eq",
            format!(
                "{w}enum Pick<U> {{\n    One(U)\n}}\nextend<T: Eq> W<T>: Eq {{\n    fn eq(self, other: Self) -> bool {{\n        Pick.One(W(x: [self.x])) == Pick.One(W(x: [other.x]))\n    }}\n}}\n{main}"
            ),
            Some("Pick.One(W(x: [self.x])) == Pick.One(W(x: [other.x]))"),
        ),
        (
            "an array of an operand that grows, reached through the hub",
            format!(
                "{w}extend<T: Eq> W<T>: Eq {{\n    fn eq(self, other: Self) -> bool {{\n        [W(x: [self.x])] == [W(x: [other.x])]\n    }}\n}}\n{main}"
            ),
            Some("[W(x: [self.x])] == [W(x: [other.x])]"),
        ),
        (
            "an operand that does not grow",
            format!(
                "{w}extend<T: Eq> W<T>: Eq {{\n    fn eq(self, other: Self) -> bool {{\n        [self.x] == [other.x]\n    }}\n}}\n{main}"
            ),
            None,
        ),
    ];
    for (row, src, cycle) in rows {
        match cycle {
            Some(text) => codes_row(
                row,
                &src,
                &[(Code::InstantiationCycle, where_is(&src, text))],
            ),
            None => clean_row(row, &src),
        }
    }
}

#[test]
fn implicit_print_should_close_an_instantiation_cycle() {
    let w = "class W<T> {\n    x: T\n}\n";
    let main = "fn main() {\n    print(W(x: 1))\n}";
    let growing = "print(W(x: [self.x]))";
    let rows = [
        (
            "a print of a class that grows",
            format!(
                "{w}extend<T> W<T>: Display {{\n    fn to_string(self) -> string {{\n        {growing}\n        \"w\"\n    }}\n}}\n{main}"
            ),
            Some(growing),
        ),
        (
            "a print of an enum payload, reached through its generated Display",
            format!(
                "{w}enum Pick<U> {{\n    One(U)\n}}\nextend<T> W<T>: Display {{\n    fn to_string(self) -> string {{\n        print(Pick.One(W(x: [self.x])))\n        \"w\"\n    }}\n}}\n{main}"
            ),
            Some("print(Pick.One(W(x: [self.x])))"),
        ),
        (
            "a print that does not grow",
            format!(
                "{w}extend<T> W<T>: Display {{\n    fn to_string(self) -> string {{\n        print([self.x])\n        \"w\"\n    }}\n}}\n{main}"
            ),
            None,
        ),
    ];
    for (row, src, cycle) in rows {
        match cycle {
            Some(text) => codes_row(
                row,
                &src,
                &[(Code::InstantiationCycle, where_is(&src, text))],
            ),
            None => clean_row(row, &src),
        }
    }
}

/// The type that grows in the field of `Hold<T>`, which `dbg_cycle_rows` declares first. A cycle
/// that the field type makes is reported there, whatever the operand that reaches it.
const FIELD: &str = "W<[T]>";

/// The same type in the field of `Rec<T>`, which follows `Hold<T>`.
const REC_FIELD: &str = "W<[T]>\n}\nenum Pick";

/// `dbg` shows the fields of a class with no written `to_string`. So a growing value held in a
/// field, in an array or in an enum payload is a cycle for `dbg`. A class whose field type grows
/// is reported at the field. A class that only passes its parameter on to a field is reported at
/// the `dbg`, where the argument grows.
#[test]
fn implicit_dbg_should_close_an_instantiation_cycle() {
    dbg_cycle_rows(&[
        (
            "a dbg of a class that grows",
            "dbg(W(x: [self.x]))",
            Some("dbg(W(x: [self.x]))"),
        ),
        (
            "a dbg of a class that holds a class that grows",
            "let _ = dbg(Wrap(v: W(x: [self.x])))",
            Some("dbg(Wrap(v: W(x: [self.x])))"),
        ),
        (
            "a dbg of an array of such a class",
            "let _ = dbg([Wrap(v: W(x: [self.x]))])",
            Some("dbg([Wrap(v: W(x: [self.x]))])"),
        ),
        (
            "a dbg of an enum payload of such a class",
            "let _ = dbg(Pick.One(Wrap(v: W(x: [self.x]))))",
            Some("dbg(Pick.One(Wrap(v: W(x: [self.x]))))"),
        ),
        (
            "a dbg of an enum payload that holds the class twice",
            "let _ = dbg(Pair.Two(Wrap(v: W(x: [self.x])), Wrap(v: W(x: [self.x]))))",
            Some("dbg(Pair.Two(Wrap(v: W(x: [self.x])), Wrap(v: W(x: [self.x]))))"),
        ),
        // `Hold<T>` itself grows nothing: only its field type `W<[T]>` does.
        (
            "a dbg of a class whose field type grows",
            "let _ = dbg(Hold(v: W(x: [self.x])))",
            Some(FIELD),
        ),
        (
            "a dbg of an array of such a class",
            "let _ = dbg([Hold(v: W(x: [self.x]))])",
            Some(FIELD),
        ),
        (
            "a dbg of an optional of such a class",
            "let _ = dbg(Some(Hold(v: W(x: [self.x]))))",
            Some(FIELD),
        ),
        (
            "a dbg of an enum payload of such a class",
            "let _ = dbg(Pick.One(Hold(v: W(x: [self.x]))))",
            Some(FIELD),
        ),
        (
            "a dbg of a recursive class whose field type grows",
            "let _ = dbg(Rec(next: None, v: W(x: [self.x])))",
            Some(REC_FIELD),
        ),
        (
            "a dbg of an array of recursive classes whose field type grows",
            "let _ = dbg([Rec(next: None, v: W(x: [self.x]))])",
            Some(REC_FIELD),
        ),
        // `Wrap<T>` passes its parameter to the field `W<T>`: the argument grows at the `dbg`.
        (
            "a dbg of an enum payload that is a class which passes its parameter on",
            "let _ = dbg(Pick.One(Wrap(v: W(x: [self.x]))))",
            Some("dbg(Pick.One(Wrap(v: W(x: [self.x]))))"),
        ),
        (
            "a print of a class whose field type grows",
            "print(Hold(v: W(x: [self.x])))",
            None,
        ),
        ("a dbg that does not grow", "dbg(self.x)", None),
    ]);
}

/// The same through an enum: a payload that holds the class, a recursive enum, and an enum that
/// shows no argument. A class with a field that grows is reported at its field, however the
/// enum holds it. A payload that grows, or a class that passes its parameter on, is reported at
/// the payload. That class inside another enum is reported at its field, which does not grow.
#[test]
fn implicit_dbg_should_close_an_instantiation_cycle_through_an_enum() {
    dbg_cycle_rows(&[
        (
            "a dbg of a recursive enum that holds such a class",
            "let _ = dbg(Tree.Leaf(Hold(v: W(x: [self.x]))))",
            Some(FIELD),
        ),
        (
            "a dbg of an array of a recursive enum that holds such a class",
            "let _ = dbg([Tree.Leaf(Hold(v: W(x: [self.x])))])",
            Some(FIELD),
        ),
        (
            "a dbg of an enum that holds the class in an array",
            "let _ = dbg(Bag.Many([Hold(v: W(x: [self.x]))]))",
            Some(FIELD),
        ),
        (
            "a dbg of an enum that holds the class in an optional",
            "let _ = dbg(Bag.Maybe(Some(Hold(v: W(x: [self.x])))))",
            Some(FIELD),
        ),
        // No payload holds `P`, so `dbg` shows nothing of the argument `Hold<T>`.
        (
            "a dbg of an enum that holds no value of the class",
            "let p: Phantom<i64, Hold<T>> = Phantom.One(1)\n        dbg(p)",
            None,
        ),
        // An argument of an enum inside the argument of another is shown when both show it.
        (
            "a dbg of an enum that holds the class in the argument of an argument",
            "let _ = dbg(Nest.One(Pick.One(Pick.One(Hold(v: W(x: [self.x]))))))",
            Some(FIELD),
        ),
        (
            "a dbg of an enum that names the class in an argument that is not shown",
            "let p: NestPhantom<Hold<T>> = NestPhantom.One(Phantom.One(1))\n        dbg(p)",
            None,
        ),
        // The payload type gives the class a longer argument: the growth is at the payload.
        (
            "a dbg of an enum whose payload type grows",
            "let _ = dbg(Grow.One(Wrap(v: W(x: [self.x]))))",
            Some("Wrap<[U]>"),
        ),
        // The class is named in the payload, and its field type grows.
        (
            "a dbg of an enum whose payload is such a class",
            "let _ = dbg(Both.One(Hold(v: W(x: [self.x]))))",
            Some(FIELD),
        ),
        // The class passes its parameter to a field: the enum is reported, at the payload.
        (
            "a dbg of an enum whose payload is a class which passes its parameter on",
            "let _ = dbg(PassOn.One(Wrap(v: W(x: [self.x]))))",
            Some("Wrap<U>"),
        ),
        // The class is in another enum: it is reported at its field type `W<T>`, which does not
        // grow. The text `W<T>\n}` skips `class W<T>` and starts at that field type.
        (
            "a dbg of an enum whose payload holds such a class inside another enum",
            "let _ = dbg(NestW.One(Pick.One(Wrap(v: W(x: [self.x])))))",
            Some("W<T>\n}"),
        ),
    ]);
}

/// Checks each row `(name, body, cycle)`: the program whose `Display` of `W<T>` runs `body`
/// reports an E0140 at the text `cycle`, or checks clean when it is `None`.
fn dbg_cycle_rows(rows: &[(&str, &str, Option<&str>)]) {
    let w = "class W<T> {\n    x: T\n}\nclass Wrap<T> {\n    v: W<T>\n}\nclass Hold<T> {\n    v: W<[T]>\n}\nclass Rec<T> {\n    next: Rec<T>?\n    v: W<[T]>\n}\nenum Pick<U> {\n    One(U)\n}\nenum Pair<U> {\n    Two(U, U)\n}\nenum Tree<U> {\n    Leaf(U)\n    Node(Tree<U>, Tree<U>)\n}\nenum Both<U> {\n    One(Hold<U>)\n}\nenum PassOn<U> {\n    One(Wrap<U>)\n}\nenum NestW<U> {\n    One(Pick<Wrap<U>>)\n}\nenum Grow<U> {\n    One(Wrap<[U]>)\n}\nenum Nest<U> {\n    One(Pick<Pick<U>>)\n}\nenum NestPhantom<U> {\n    One(Phantom<i64, Pick<U>>)\n}\nenum Phantom<U, P> {\n    One(U)\n}\nenum Bag<U> {\n    Many([U])\n    Maybe(U?)\n}\n";
    let main = "fn main() {\n    print(W(x: 1))\n}";
    let display = |body: &str| {
        format!(
            "{w}extend<T> W<T>: Display {{\n    fn to_string(self) -> string {{\n        {body}\n        \"w\"\n    }}\n}}\n{main}"
        )
    };
    for &(row, body, cycle) in rows {
        let src = display(body);
        match cycle {
            Some(text) => codes_row(
                row,
                &src,
                &[(Code::InstantiationCycle, where_is(&src, text))],
            ),
            None => clean_row(row, &src),
        }
    }
}

/// `dbg` shows the text of a class or an enum with a written `Display` that always applies, so it
/// follows no field or payload of it. The type `Hold<T>` holds a type `W<[T]>`, which grows. A
/// bound on another interface may fail, or the extension may be for one type argument only. Then
/// `dbg` shows the fields, so those edges stay.
#[test]
fn dbg_should_follow_nothing_of_a_class_or_an_enum_with_an_applicable_written_display() {
    let decls = "interface Mark {\n    fn mark(self) -> string\n}\nclass W<T> {\n    x: T\n}\nenum Pick<U> {\n    One(U)\n}\n";
    let text = "fn to_string(self) -> string {\n        \"hold\"\n    }\n";
    let class = "class Hold<T> {\n    v: W<[T]>\n";
    let enum_ = "class Plain<T> {\n    v: W<[T]>\n}\nenum Hold<T> {\n    One(Plain<T>)\n";
    let display = |hold: &str, body: &str| {
        format!(
            "{decls}{hold}extend<T> W<T>: Display {{\n    fn to_string(self) -> string {{\n        {body}\n        \"w\"\n    }}\n}}\nfn main() {{\n    print(W(x: 1))\n}}"
        )
    };
    // The subject and how to make it. A cycle is reported at the field type that grows.
    let subjects = [
        ("class", class, "Hold(v: W(x: [self.x]))"),
        ("enum", enum_, "Hold.One(Plain(v: W(x: [self.x])))"),
    ];
    for (subject, open, made) in subjects {
        let extended =
            |header: &str| format!("{open}}}\nextend{header} Hold<T>: Display {{\n    {text}}}\n");
        let shapes = [
            ("itself", format!("dbg({made})")),
            ("an array", format!("dbg([{made}])")),
            ("an optional", format!("dbg(Some({made}))")),
            ("an enum payload", format!("dbg(Pick.One({made}))")),
            ("an array in an optional", format!("dbg(Some([{made}]))")),
        ];
        let mut writtens = vec![
            ("an extension with no bound", extended("<T>"), false),
            (
                "an extension bounded by Display",
                extended("<T: Display>"),
                false,
            ),
            (
                "an extension bounded by another interface",
                extended("<T: Mark>"),
                true,
            ),
            (
                "an extension for one type argument",
                {
                    // `extend Hold<i64>` has no `T` to bind, so it cannot apply to `Hold<T>`.
                    format!("{open}}}\nextend Hold<i64>: Display {{\n    {text}}}\n")
                },
                true,
            ),
            ("no written Display", format!("{open}}}\n"), true),
        ];
        if subject == "class" {
            // Only a class can write the method in its body.
            writtens.push((
                "a method of the class",
                format!("{open}    {text}}}\n"),
                false,
            ));
        }
        for (written, hold, cycle) in &writtens {
            for (shape, body) in &shapes {
                let row = format!("{shape} of {subject} with {written}");
                let src = display(hold, body);
                if *cycle {
                    let at = where_is(&src, FIELD);
                    codes_row(&row, &src, &[(Code::InstantiationCycle, at)]);
                } else {
                    clean_row(&row, &src);
                }
            }
        }
    }
}

/// A field or a payload whose type did not check has its own error, and adds no edge.
#[test]
fn dbg_should_skip_a_field_and_a_payload_that_did_not_check() {
    for (row, decl, value) in [
        (
            "a field",
            "class Bad<T> {\n    v: Missing\n    w: T\n}",
            "Bad(v: 1, w: 2)",
        ),
        (
            "a payload",
            "enum Bad<T> {\n    A(Missing)\n    B(T)\n}",
            "Bad.B(1)",
        ),
    ] {
        let src = format!(
            "{decl}\nfn look<T>(x: Bad<T>) {{\n    dbg(x)\n}}\nfn main() {{\n    look({value})\n}}"
        );
        let at = where_is(&src, "Missing");
        codes_row(row, &src, &[(Code::UnknownType, at)]);
    }
}

/// A bare parameter shows the class that it gets, and an enum with a written `Display` that always
/// applies shows its text. So the argument of the enum is read for the first, not for the second.
#[test]
fn dbg_should_read_no_argument_of_an_enum_with_a_written_display() {
    let reads = |slot: &str| {
        let src = format!(
            "class Hold<T> {{\n    v: T\n}}\nenum Slot<U> {{\n    Full(U)\n}}\n{slot}enum Outer<U> {{\n    One(Slot<U>)\n}}\nfn look<T>(x: Outer<Hold<T>>) {{\n    dbg(x)\n}}\nfn main() {{\n    look(Outer.One(Slot.Full(Hold(v: 1))))\n}}"
        );
        cycle::DBG_VISITS.set(0);
        clean(&src);
        cycle::DBG_VISITS.get()
    };
    let written = "extend<U> Slot<U>: Display {\n    fn to_string(self) -> string {\n        \"slot\"\n    }\n}\n";
    // The operand `Outer<Hold<T>>` and the payload `U` in the node of `Slot`, not `Hold<T>`.
    assert_eq!(reads(written), 2, "a written Display shows no argument");
    // The same, and `Hold<T>`, and the payload `Slot<U>` of `Outer` with its argument.
    assert_eq!(reads(""), 5, "a generated Display shows the argument");
}

/// The reads of a `dbg` follow its operands and the enums it reaches, not their product. A chain
/// of `n` generic enums is read once for the program, by the node of each enum. The operands of
/// many types over one chain, in many functions, each read the same number of types whatever `n`
/// is. A `dbg` of a type with no parameter reads none of its own.
#[test]
fn dbg_should_read_the_types_of_an_operand_once() {
    // `holders` generic functions, each with `operands` parameters `x{i}: E0<T{i}>?` and a marker
    // that names `T{i}`, and `calls` calls of `dbg` on each. The enums chain to `n`: `E0<T>` names
    // a parameter, and `E0<i64>` none.
    let read = |n: usize, holders: usize, operands: usize, calls: usize, argument: &str| {
        let mut src = String::new();
        for k in 0..n {
            let next = if k + 1 < n {
                format!("Next(E{}<T>)", k + 1)
            } else {
                "Last(T)".to_string()
            };
            writeln!(src, "enum E{k}<T> {{\n    {next}\n}}")
                .expect("writing to a `String` should succeed");
        }
        let params = |name: &str| {
            (0..operands)
                .map(|i| format!("{name}{i}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        for h in 0..holders {
            let list = (0..operands)
                .map(|i| {
                    format!(
                        "x{i}: {}?, m{i}: T{i}",
                        argument.replace('#', &format!("T{i}"))
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(src, "fn inspect{h}<{}>({list}) {{", params("T"))
                .expect("writing to a `String` should succeed");
            for i in 0..operands {
                writeln!(src, "    let _ = m{i}").expect("writing to a `String` should succeed");
                src += &format!("    dbg(x{i})\n").repeat(calls);
            }
            src += "}\n";
        }
        src += "fn main() {\n    let x: E0<i64>? = None\n";
        for h in 0..holders {
            let list = vec!["x, 1"; operands].join(", ");
            writeln!(src, "    inspect{h}({list})").expect("writing to a `String` should succeed");
        }
        src += "}";
        cycle::DBG_VISITS.set(0);
        clean(&src);
        cycle::DBG_VISITS.get()
    };
    let one = |n: usize| read(n, 1, 1, 1, "E0<#>");
    let (short, long) = (30, 60);
    let chain = one(long) - one(short);
    assert!(
        chain >= long - short,
        "the chain should be read: {chain} types for {} more enums",
        long - short
    );
    assert!(chain <= 3 * (long - short), "{chain} types for the chain");
    // Many operand types over one chain: the same added reads for a short and a long chain.
    let operands = |n: usize, calls: usize| read(n, 1, 20, calls, "E0<#>") - one(n);
    let many = operands(short, 1);
    assert!(many > 0, "each operand type should be read");
    assert_eq!(
        operands(long, 1),
        many,
        "20 operand types should not read the chain again"
    );
    assert_eq!(
        operands(short, 5),
        many,
        "5 calls of each operand read no more than one"
    );
    // A holder reads an operand type once, so each function reads it, but not the chain again.
    let functions = |n: usize| read(n, 20, 1, 1, "E0<#>") - one(n);
    assert!(functions(short) > 0, "each function should read the type");
    assert_eq!(
        functions(long),
        functions(short),
        "20 functions should not read the chain again"
    );
    // The chain is read for any `dbg`; an operand with no parameter adds nothing to it.
    let closed = read(short, 1, 1, 1, "E0<i64>");
    assert!(
        closed < one(short),
        "an operand with no parameter reads less"
    );
    assert_eq!(
        read(short, 5, 5, 5, "E0<i64>"),
        closed,
        "an operand with no parameter reads nothing"
    );
}

/// A `dbg` adds the edges of its operand's fields once for a function, whatever number of calls
/// name the type: the calls after the first add what a `print` of the operand adds. The enums
/// of a chain each hold a class whose field type is a class of its own.
#[test]
fn dbg_should_add_the_edges_of_the_fields_once_for_a_function() {
    let edges = |shown: &str, calls: usize| {
        let n = 30;
        let mut src = String::new();
        for k in 0..n {
            let next = if k + 1 < n {
                format!("    Next(E{}<T>, C{k}<T>)\n", k + 1)
            } else {
                String::new()
            };
            writeln!(
                src,
                "class Tag{k}<T> {{\n    x: T\n}}\nclass C{k}<T> {{\n    v: Tag{k}<T>\n}}\nenum E{k}<T> {{\n    End(T)\n{next}}}"
            )
            .expect("writing to a `String` should succeed");
        }
        src += "fn inspect<T>(x: E0<T>) {\n";
        src += &format!("    {shown}(x)\n").repeat(calls);
        src += "}\nfn main() {\n    let x = E0.End(1)\n    inspect(x)\n}";
        cycle::EDGES.set(0);
        clean(&src);
        cycle::EDGES.get()
    };
    let extra = |calls: usize| edges("dbg", calls) - edges("print", calls);
    assert!(extra(1) > 0, "a dbg adds the edges of the fields");
    assert_eq!(extra(40), extra(1), "40 calls add them once");
}

/// The edges of a `dbg` follow the operands and the enums, not their product: `n` enums in a chain
/// give their edges to the `dbg` node of each enum, and an operand of one of `k` types adds the
/// same edges whatever `n` is.
#[test]
fn dbg_should_add_edges_in_proportion_to_the_operands_and_the_enums() {
    let edges = |n: usize, k: usize| {
        let mut src = String::new();
        for i in 0..n {
            let next = if i + 1 < n {
                format!("Next(E{}<T>)", i + 1)
            } else {
                "Last(T)".to_string()
            };
            writeln!(src, "enum E{i}<T> {{\n    {next}\n}}")
                .expect("writing to a `String` should succeed");
        }
        let params: Vec<_> = (0..k).map(|i| format!("T{i}")).collect();
        let list: Vec<_> = (0..k)
            .map(|i| format!("x{i}: E0<T{i}>?, m{i}: T{i}"))
            .collect();
        writeln!(
            src,
            "fn inspect<{}>({}) {{",
            params.join(", "),
            list.join(", ")
        )
        .expect("writing to a `String` should succeed");
        for i in 0..k {
            writeln!(src, "    let _ = m{i}\n    dbg(x{i})")
                .expect("writing to a `String` should succeed");
        }
        src += "}\nfn main() {\n    let x: E0<i64>? = None\n";
        writeln!(src, "    inspect({})\n}}", vec!["x, 1"; k].join(", "))
            .expect("writing to a `String` should succeed");
        cycle::EDGES.set(0);
        clean(&src);
        cycle::EDGES.get()
    };
    let (short, long) = (30, 60);
    let chain = edges(long, 1) - edges(short, 1);
    assert!(
        chain > 0,
        "the enums add edges, the `dbg` node of each among them"
    );
    assert_eq!(
        edges(long, 20) - edges(short, 20),
        chain,
        "20 operand types should not add the edges of the chain again"
    );
}

/// A class has one generated `dbg` node, so the edges of its fields are added once for the
/// program: whatever number of functions and of operand types name the class, a field adds the
/// same edges. `fields` fields of the class `C<T>`, and `holders` generic functions with
/// `operands` parameters of type `C<T{i}>`, each shown by a `dbg`.
#[test]
fn dbg_should_add_the_edges_of_the_fields_of_a_class_once_for_the_program() {
    let edges = |holders: usize, operands: usize, fields: usize| {
        let mut src = String::from("class C<T> {\n");
        for f in 0..fields {
            writeln!(src, "    f{f}: [T]").expect("writing to a `String` should succeed");
        }
        src += "}\n";
        let params: Vec<_> = (0..operands).map(|i| format!("T{i}")).collect();
        let list: Vec<_> = (0..operands).map(|i| format!("a{i}: C<T{i}>")).collect();
        for h in 0..holders {
            writeln!(
                src,
                "fn inspect{h}<{}>({}) {{",
                params.join(", "),
                list.join(", ")
            )
            .expect("writing to a `String` should succeed");
            for i in 0..operands {
                writeln!(src, "    dbg(a{i})").expect("writing to a `String` should succeed");
            }
            src += "}\n";
        }
        let empty: Vec<_> = (0..fields).map(|f| format!("f{f}: []")).collect();
        writeln!(
            src,
            "fn main() {{\n    let c: C<i64> = C({})",
            empty.join(", ")
        )
        .expect("writing to a `String` should succeed");
        for h in 0..holders {
            writeln!(src, "    inspect{h}({})", vec!["c"; operands].join(", "))
                .expect("writing to a `String` should succeed");
        }
        src += "}";
        cycle::EDGES.set(0);
        clean(&src);
        cycle::EDGES.get()
    };
    let (few, many) = (10, 20);
    let field_edges = |holders: usize, operands: usize| {
        edges(holders, operands, many) - edges(holders, operands, few)
    };
    let once = field_edges(1, 1);
    assert!(once > 0, "the fields should add edges");
    assert_eq!(field_edges(10, 1), once, "10 functions share the fields");
    assert_eq!(
        field_edges(1, 10),
        once,
        "10 operand types share the fields"
    );
    assert_eq!(field_edges(10, 10), once, "100 operands share the fields");
}

/// The arguments that `dbg` shows are found in one pass over the payloads of the enums: a chain of
/// `n` parameters that each pass to the next costs the same for each one.
#[test]
fn dbg_should_find_the_shown_arguments_in_proportion_to_the_type_parameters() {
    // `E<T0, .., Tn-1>` holds `T0` and `E<T1, .., Tn-1, T0>`: a parameter becomes shown at a time.
    let steps = |n: usize| {
        let params: Vec<_> = (0..n).map(|i| format!("T{i}")).collect();
        let same = vec!["T"; n].join(", ");
        let rotated: Vec<_> = (1..=n).map(|i| format!("T{}", i % n)).collect();
        let closed = vec!["i64"; n].join(", ");
        let src = format!(
            "enum E<{}> {{\n    Here(T0)\n    Next(E<{}>)\n}}\nfn look<T>(x: E<{same}>, m: T) {{\n    let _ = m\n    dbg(x)\n}}\nfn main() {{\n    let e: E<{closed}> = E.Here(1)\n    look(e, 1)\n}}",
            params.join(", "),
            rotated.join(", ")
        );
        cycle::SHOWN_STEPS.set(0);
        clean(&src);
        cycle::SHOWN_STEPS.get()
    };
    let (a, b, c) = (steps(20), steps(40), steps(60));
    assert!(a > 0, "the payloads should be read");
    assert_eq!(
        c - b,
        b - a,
        "each parameter should cost the same: {a} {b} {c}"
    );
}

#[test]
fn implicit_interpolation_should_close_an_instantiation_cycle() {
    let w = "class W<T> {\n    x: T\n}\n";
    let main = "fn main() {\n    print(W(x: 1))\n}";
    let growing = "\"{W(x: [self.x])}\"";
    let rows = [
        (
            "an interpolation of a class that grows",
            format!(
                "{w}extend<T> W<T>: Display {{\n    fn to_string(self) -> string {{\n        {growing}\n    }}\n}}\n{main}"
            ),
            // The error stands at the part of the string, not at the whole string.
            Some("W(x: [self.x])"),
        ),
        (
            "an interpolation of an enum payload, reached through its generated Display",
            format!(
                "{w}enum Pick<U> {{\n    One(U)\n}}\nextend<T> W<T>: Display {{\n    fn to_string(self) -> string {{\n        \"{{Pick.One(W(x: [self.x]))}}\"\n    }}\n}}\n{main}"
            ),
            Some("Pick.One(W(x: [self.x]))"),
        ),
        (
            "an interpolation that does not grow",
            format!(
                "{w}extend<T> W<T>: Display {{\n    fn to_string(self) -> string {{\n        \"{{[self.x]}}\"\n    }}\n}}\n{main}"
            ),
            None,
        ),
    ];
    for (row, src, cycle) in rows {
        match cycle {
            Some(text) => codes_row(
                row,
                &src,
                &[(Code::InstantiationCycle, where_is(&src, text))],
            ),
            None => clean_row(row, &src),
        }
    }
}

/// A `to_string` call takes the edges of an interpolation of its receiver (see
/// `implicit_interpolation_should_close_an_instantiation_cycle`): a real cycle is the same error.
#[test]
fn to_string_call_should_close_an_instantiation_cycle() {
    let w = "class W<T> {\n    x: T\n}\n";
    let main = "fn main() {\n    print(W(x: 1))\n}";
    let growing = "W(x: [self.x]).to_string()";
    let rows = [
        (
            "a to_string of a class that grows",
            format!(
                "{w}extend<T> W<T>: Display {{\n    fn to_string(self) -> string {{\n        {growing}\n    }}\n}}\n{main}"
            ),
            Some(growing.to_string()),
        ),
        (
            "a to_string of an enum payload, reached through its generated Display",
            format!(
                "{w}enum Pick<U> {{\n    One(U)\n}}\nextend<T> W<T>: Display {{\n    fn to_string(self) -> string {{\n        Pick.One(W(x: [self.x])).to_string()\n    }}\n}}\n{main}"
            ),
            Some("Pick.One(W(x: [self.x])).to_string()".to_string()),
        ),
        (
            "a to_string of a parameter that holds a class that grows",
            format!(
                "{w}fn text<A>(a: A) -> string {{\n    a.to_string()\n}}\nextend<T> W<T>: Display {{\n    fn to_string(self) -> string {{\n        text(W(x: [self.x]))\n    }}\n}}\n{main}"
            ),
            Some("text(W(x: [self.x]))".to_string()),
        ),
        (
            "a to_string that does not grow",
            format!(
                "{w}extend<T> W<T>: Display {{\n    fn to_string(self) -> string {{\n        [self.x].to_string()\n    }}\n}}\n{main}"
            ),
            None,
        ),
    ];
    for (row, src, cycle) in rows {
        match cycle {
            Some(text) => codes_row(
                row,
                &src,
                &[(Code::InstantiationCycle, where_is(&src, &text))],
            ),
            None => clean_row(row, &src),
        }
    }
}

#[test]
fn operand_under_arrays_of_a_parameter_should_not_close_a_cycle() {
    let same = "fn same<A: Eq>(a: A, b: A) -> bool {\n    a == b\n}\n";
    let rows = [
        (
            "arrays of the parameter, through a bare parameter's second node",
            format!("{same}class Grid<T> {{\n    rows: [[T]]\n}}\nextend<T: Eq> Grid<T>: Eq {{\n    fn eq(self, other: Self) -> bool {{\n        same(self.rows, other.rows)\n    }}\n}}\nfn main() {{\n    print(Grid(rows: [[1]]) == Grid(rows: [[2]]))\n}}"),
        ),
        (
            "arrays under an enum's generated Eq",
            "class G2<T> {\n    x: T\n}\nenum Q<V> {\n    A(V)\n}\nextend<T: Eq> G2<T>: Eq {\n    fn eq(self, other: Self) -> bool {\n        Q.A([[self.x]]) == Q.A([[other.x]])\n    }\n}\nfn main() {\n    print(G2(x: 1) == G2(x: 2))\n}".to_string(),
        ),
        (
            "arrays of the parameter, interpolated through a bare parameter's second node",
            "class Grid<T> {\n    rows: [[T]]\n}\nfn text<A>(a: A) -> string {\n    \"{a}\"\n}\nextend<T> Grid<T>: Display {\n    fn to_string(self) -> string {\n        text(self.rows)\n    }\n}\nfn main() {\n    print(Grid(rows: [[1]]))\n}".to_string(),
        ),
        (
            "arrays of the parameter, through the to_string of a bare parameter's second node",
            "class Grid<T> {\n    rows: [[T]]\n}\nfn text<A>(a: A) -> string {\n    a.to_string()\n}\nextend<T> Grid<T>: Display {\n    fn to_string(self) -> string {\n        text(self.rows)\n    }\n}\nfn main() {\n    print(Grid(rows: [[1]]))\n}".to_string(),
        ),
        (
            "arrays of the parameter, printed through a bare parameter's second node",
            "class Grid<T> {\n    rows: [[T]]\n}\nfn show<A>(a: A) {\n    print(a)\n}\nextend<T> Grid<T>: Display {\n    fn to_string(self) -> string {\n        show(self.rows)\n        \"g\"\n    }\n}\nfn main() {\n    print(Grid(rows: [[1]]))\n}".to_string(),
        ),
        (
            "a second node that comes from the node of the element, not of the whole argument",
            format!("{same}class Tree<T> {{\n    value: T\n    children: [Tree<T>]\n}}\nextend<T: Eq> Tree<T>: Eq {{\n    fn eq(self, other: Self) -> bool {{\n        self.value == other.value && same(self.children, other.children)\n    }}\n}}\nfn main() {{\n    print(Tree(value: 1, children: []) == Tree(value: 2, children: []))\n}}"),
        ),
    ];
    for (row, src) in rows {
        clean_row(row, &src);
    }
}

#[test]
fn growing_declaration_should_give_one_diagnostic() {
    let nest = "enum Nest<T> {\n    End\n    Next(Nest<[T]>)\n}\nfn f(a: Nest<i64>, b: Nest<i64>) -> bool {\n    a == b\n}\nfn p(n: Nest<i64>) {\n    print(n)\n}\nfn t(n: Nest<i64>) -> string {\n    \"{n}\"\n}\nfn main() {}";
    codes(
        nest,
        &[(Code::InstantiationCycle, where_is(nest, "Nest<[T]>"))],
    );
    // An enum no value can be built of has its own error alone, not an E0140 beside it.
    let dead = "enum Dead<T> {\n    Again(Dead<[T]>)\n}\nfn f(a: Dead<i64>, b: Dead<i64>) -> bool {\n    a == b\n}\nfn main() {}";
    codes(dead, &[(Code::NeverConstructed, where_is(dead, "Dead"))]);
}

#[test]
fn eq_and_less_calls_should_check_their_receiver_and_argument() {
    // Each row: the program, the text where the error stands, and its code.
    let rows = [
        (
            "an argument of the wrong type",
            "fn f() -> bool { 5.eq(\"a\") }\nfn main() {}",
            "\"a\"",
            Code::Mismatch,
        ),
        (
            "an argument that fails leaves no type to check",
            "fn f() {\n    let x: i64 = 5.eq(\"a\")\n}\nfn main() {}",
            "\"a\"",
            Code::Mismatch,
        ),
        (
            "a receiver that is unknown",
            "fn f() -> bool { missing.eq(1) }\nfn main() {}",
            "missing",
            Code::BindingNotFound,
        ),
        (
            "two arguments are too many",
            "fn f() -> bool { 5.eq(5, 6) }\nfn main() {}",
            "5.eq",
            Code::ArgumentCount,
        ),
        (
            "no argument is too few",
            "fn f() -> bool { 5.less() }\nfn main() {}",
            "5.less",
            Code::ArgumentCount,
        ),
        (
            "a label is no label of a method",
            "fn f() -> bool { 5.eq(other: 5) }\nfn main() {}",
            "other",
            Code::LabeledArgument,
        ),
        (
            "a label on an enum's generated `eq`",
            "enum E {\n    A\n}\nfn f(a: E) -> bool { a.eq(other: a) }\nfn main() {}",
            "other",
            Code::LabeledArgument,
        ),
    ];
    for (row, src, at, code) in rows {
        codes_row(row, src, &[(code, where_is(src, at))]);
    }
}

#[test]
fn a_written_eq_or_less_method_should_win_over_the_generated_one() {
    // Each row: a type that writes a method of the name and has no `: Eq` or `: Ord`, so the
    // method takes a call of that name, whatever its signature (ADR 0023).
    let rows = [
        (
            "an inherent `eq` of an enum",
            "enum E {\n    A\n    fn eq(self, n: i64) -> bool { n == 0 }\n}\nfn f(x: E) -> bool { x.eq(0) }\nfn main() {}",
        ),
        (
            "an `eq` of a plain `extend` of an enum",
            "enum E {\n    A\n}\nextend E {\n    fn eq(self, n: i64) -> bool { n == 0 }\n}\nfn f(x: E) -> bool { x.eq(0) }\nfn main() {}",
        ),
        (
            "an `eq` of a generic enum",
            "enum P<T> {\n    A(T)\n    fn eq(self, n: i64) -> bool { n == 0 }\n}\nfn f(x: P<i64>) -> bool { x.eq(0) }\nfn main() {}",
        ),
        (
            "an `eq` that a conformance of another interface writes",
            "interface Same {\n    fn eq(self, n: i64) -> bool\n}\nenum E {\n    A\n}\nextend E: Same {\n    fn eq(self, n: i64) -> bool { n == 0 }\n}\nfn f(x: E) -> bool { x.eq(0) }\nfn main() {}",
        ),
        (
            "a `less` of an enum",
            "enum E {\n    A\n    fn less(self, n: i64) -> bool { n == 0 }\n}\nfn f(x: E) -> bool { x.less(0) }\nfn main() {}",
        ),
        (
            "an `eq` for `P<i64>` called on a `P<i64>`",
            "enum P<T> {\n    A(T)\n}\nextend P<i64> {\n    fn eq(self, n: i64) -> bool { n == 0 }\n}\nfn f(x: P<i64>) -> bool { x.eq(0) }\nfn main() {}",
        ),
        (
            "an `eq` for `P<i64>` called on a `P<string>`: the generated one",
            "enum P<T> {\n    A(T)\n}\nextend P<i64> {\n    fn eq(self, n: i64) -> bool { n == 0 }\n}\nfn f(x: P<string>) -> bool { x.eq(P.A(\"s\")) }\nfn main() {}",
        ),
        (
            "an `eq` of a conformance for `P<i64>` called on a `P<string>`",
            "interface Same {\n    fn eq(self, n: i64) -> bool\n}\nenum P<T> {\n    A(T)\n}\nextend P<i64>: Same {\n    fn eq(self, n: i64) -> bool { n == 0 }\n}\nfn f(x: P<string>) -> bool { x.eq(P.A(\"s\")) }\nfn main() {}",
        ),
    ];
    for (row, src) in rows {
        clean_row(row, src);
    }
}

#[test]
fn a_written_eq_or_less_whose_block_bounds_fail_should_report_the_bound() {
    // Each row: a block that covers the receiver `P<i64>` (or `Pair<bool>`) and fails its bounds.
    // The method counts as written, so the call reports the bound; the generated `eq` or `less`
    // is no fallback (only `to_string` has one).
    let flag = "interface Flag {}\ninterface Same {\n    fn eq(self, n: i64) -> bool\n    fn less(self, n: i64) -> bool\n}\n";
    let p = "enum P<T> {\n    A(T)\n}\n";
    let rows = [
        (
            "a plain block `eq` with its own signature",
            format!("{flag}{p}extend<T: Flag> P<T> {{\n    fn eq(self, n: i64) -> bool {{ n == 0 }}\n}}\nfn f(x: P<i64>) -> bool {{ x.eq(0) }}\nfn main() {{}}"),
            "x.eq(0)",
        ),
        (
            "a plain block `eq` with the generated signature",
            format!("{flag}{p}extend<T: Flag> P<T> {{\n    fn eq(self, other: Self) -> bool {{ true }}\n}}\nfn f(x: P<i64>) -> bool {{ x.eq(P.A(0)) }}\nfn main() {{}}"),
            "x.eq(P.A(0))",
        ),
        (
            "a plain block `less` with its own signature",
            format!("{flag}{p}extend<T: Flag> P<T> {{\n    fn less(self, n: i64) -> bool {{ n == 0 }}\n}}\nfn f(x: P<i64>) -> bool {{ x.less(0) }}\nfn main() {{}}"),
            "x.less(0)",
        ),
        (
            "a plain block `less` with the generated signature",
            format!("{flag}{p}extend<T: Flag> P<T> {{\n    fn less(self, other: Self) -> bool {{ true }}\n}}\nfn f(x: P<i64>) -> bool {{ x.less(P.A(0)) }}\nfn main() {{}}"),
            "x.less(P.A(0))",
        ),
        (
            "a conformance block `eq` of another interface",
            format!("{flag}{p}extend<T: Flag> P<T>: Same {{\n    fn eq(self, n: i64) -> bool {{ n == 0 }}\n    fn less(self, n: i64) -> bool {{ n == 0 }}\n}}\nfn f(x: P<i64>) -> bool {{ x.eq(0) }}\nfn main() {{}}"),
            "x.eq(0)",
        ),
        (
            "a conformance block `less` of another interface",
            format!("{flag}{p}extend<T: Flag> P<T>: Same {{\n    fn eq(self, n: i64) -> bool {{ n == 0 }}\n    fn less(self, n: i64) -> bool {{ n == 0 }}\n}}\nfn f(x: P<i64>) -> bool {{ x.less(0) }}\nfn main() {{}}"),
            "x.less(0)",
        ),
        (
            "a written `Eq` of a class",
            "interface Flag {}\nclass C<T> {\n    x: T\n}\nextend<T: Flag> C<T>: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\nfn f(x: C<i64>) -> bool { x.eq(x) }\nfn main() {}".to_string(),
            "x.eq(x)",
        ),
        (
            "a written `Eq` that no fallback to the generated rule replaces",
            "enum Pair<T> {\n    X(T)\n}\nextend<T: Ord> Pair<T>: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\nfn f(p: Pair<bool>, q: Pair<bool>) -> bool { p.eq(q) }\nfn main() {}".to_string(),
            "p.eq(q)",
        ),
    ];
    for (row, src, call) in rows {
        codes_row(row, &src, &[(Code::NotConform, where_is(&src, call))]);
    }
}

#[test]
fn operators_should_report_a_type_with_no_operator() {
    with_prelude();
    let node = "class Node {}\n";
    let box_wr = "class Box<T> {\n    v: T\n}\nclass Wr<T> {\n    v: T\n}\n\
        extend<T: Display> Box<T>: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\n\
        extend<T: Eq> Wr<T>: Display {\n    fn to_string(self) -> string { \"w\" }\n}\n\
        enum H {\n    P(Box<Wr<A>>)\n}\nenum A {\n    X(B)\n    Y(Node)\n}\n\
        enum B {\n    Z(A)\n    M(H)\n}\n";
    let pair = "enum Pair<T> {\n    X(T)\n}\nextend<T: Ord> Pair<T>: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\n\
        enum Outer<T> {\n    A(Pair<T>)\n}\n";
    // Each row: the declarations, the function after them, and the text where the single error
    // stands. The wording of each form is pinned in the fixtures `E0159-*`.
    let rows = [
        (
            "equality of two classes",
            format!("{node}fn f(n1: Node, n2: Node) -> bool {{ n1 == n2 }}"),
            "n1 == n2",
        ),
        (
            "inequality of two classes",
            format!("{node}fn f(n1: Node, n2: Node) -> bool {{ n1 != n2 }}"),
            "n1 != n2",
        ),
        (
            "equality of a class with an `eq` method and no `Eq`",
            "class W {\n    fn eq(self, other: Self) -> bool { true }\n}\nfn f(a: W, b: W) -> bool { a == b }".to_string(),
            "a == b",
        ),
        (
            "equality of arrays of a class",
            format!("{node}fn f(n1: Node, n2: Node) -> bool {{ [n1] == [n2] }}"),
            "[n1] == [n2]",
        ),
        (
            "equality of enums that hold a class",
            format!("{node}enum H {{\n    N(Node)\n}}\nfn f(n1: Node, n2: Node) -> bool {{ H.N(n1) == H.N(n2) }}"),
            "H.N(n1) == H.N(n2)",
        ),
        (
            "equality of enums that need each other, one of which holds a class",
            format!("{node}enum A {{\n    X(B)\n    Y(Node)\n}}\nenum B {{\n    Z(A)\n}}\nfn f(b1: B, b2: B) -> bool {{ b1 == b2 }}"),
            "b1 == b2",
        ),
        (
            "equality through a written `Display` bound",
            format!("{node}{box_wr}fn f(b1: B, b2: B) -> bool {{ b1 == b2 }}"),
            "b1 == b2",
        ),
        (
            "equality of a listed `Eq` that no method meets",
            format!("{node}enum Wrap<T>: Eq {{\n    A(T)\n}}\nfn f(n1: Node, n2: Node) -> bool {{ Wrap.A(n1) == Wrap.A(n2) }}\nfn g() -> bool {{ Wrap.A(1) == Wrap.A(2) }}"),
            "Wrap.A(n1) == Wrap.A(n2)",
        ),
        (
            "order of an enum",
            "enum E {\n    A\n    B\n}\nfn f() -> bool { E.A < E.B }".to_string(),
            "E.A < E.B",
        ),
        (
            "order of two `bool`s",
            "fn f() -> bool { true < false }".to_string(),
            "true < false",
        ),
        (
            "order of two arrays",
            "fn f() -> bool { [1] < [2] }".to_string(),
            "[1] < [2]",
        ),
        (
            "order of two classes",
            format!("{node}fn f(c1: Node, c2: Node) -> bool {{ c1 < c2 }}"),
            "c1 < c2",
        ),
        (
            "equality under a written `Eq` whose bound fails",
            format!("{pair}fn f(p: Pair<bool>, q: Pair<bool>) -> bool {{ p == q }}"),
            "p == q",
        ),
        (
            "equality of an enum that holds a written `Eq` whose bound fails",
            format!("{pair}fn f(o1: Outer<bool>, o2: Outer<bool>) -> bool {{ o1 == o2 }}"),
            "o1 == o2",
        ),
        (
            "equality of a class of a parameter",
            format!("{node}class Box<T> {{\n    v: T\n}}\nfn f<T: Eq>(a: Box<T>, b: Box<T>) -> bool {{ a == b }}"),
            "a == b",
        ),
        (
            "equality of an enum of a parameter that holds a class",
            format!("{node}enum W<T> {{\n    A(T)\n    B(W<Node>)\n}}\nfn f<T: Eq>(a: W<T>, b: W<T>) -> bool {{ a == b }}"),
            "a == b",
        ),
    ];
    for (row, src, at) in rows {
        let src = format!("{src}\nfn main() {{}}");
        codes_row(row, &src, &[(Code::NoOperator, where_is(&src, at))]);
    }
}

#[test]
fn operators_should_report_an_optional_class_with_no_operator() {
    // Each row: the function, and the text where the single error stands. A class beside its
    // optional is one type once the class is wrapped, so it has no `==` either.
    let node = "class Node {}\n";
    let rows = [
        (
            "equality of two optional classes",
            format!("{node}fn f(a: Node?, b: Node?) -> bool {{ a == b }}"),
            "a == b",
        ),
        (
            "equality of a class and its optional",
            format!("{node}fn f(a: Node, b: Node?) -> bool {{ a == b }}"),
            "a == b",
        ),
        (
            "equality of an optional class and the class",
            format!("{node}fn f(a: Node?, b: Node) -> bool {{ a == b }}"),
            "a == b",
        ),
    ];
    for (row, src, at) in rows {
        let src = format!("{src}\nfn main() {{}}");
        codes_row(row, &src, &[(Code::NoOperator, where_is(&src, at))]);
    }
}

#[test]
fn operators_should_report_a_failed_operand_alone() {
    with_prelude();
    // The other side has no type, so nothing says that the comparison needs `Eq`: only the
    // operand's own error stands. A type parameter keeps its report, as it always had.
    let node = "class Node {\n    next: Node?\n}\n";
    let rows = [
        (
            "a misspelt `None` on the right",
            format!("{node}fn f(n: Node) -> bool {{ n.next == Non }}"),
            vec![(Code::BindingNotFound, "Non")],
        ),
        (
            "an unknown name on the left",
            format!("{node}fn f(n: Node) -> bool {{ missing == n }}"),
            vec![(Code::BindingNotFound, "missing")],
        ),
        (
            "inequality with an unknown name",
            format!("{node}fn f(a: Node) -> bool {{ a != bad }}"),
            vec![(Code::BindingNotFound, "bad")],
        ),
        (
            "an optional class and an unknown name",
            format!("{node}fn f(a: Node?) -> bool {{ a == bad }}"),
            vec![(Code::BindingNotFound, "bad")],
        ),
        (
            "an array of classes and an unknown name",
            format!("{node}fn f(a: [Node]) -> bool {{ a == bad }}"),
            vec![(Code::BindingNotFound, "bad")],
        ),
        (
            "an enum that holds a class and an unknown name",
            format!("{node}enum H {{\n    N(Node)\n}}\nfn f(a: H) -> bool {{ a == bad }}"),
            vec![(Code::BindingNotFound, "bad")],
        ),
        (
            "contains of an unknown name",
            format!("{node}fn f(xs: [Node]) -> bool {{ xs.contains(bad) }}"),
            vec![(Code::BindingNotFound, "bad")],
        ),
        (
            "a type parameter and an unknown name",
            "fn f<T>(a: T) -> bool { a == bad }".to_string(),
            vec![(Code::ParamLacks, "a =="), (Code::BindingNotFound, "bad")],
        ),
    ];
    for (row, src, expected) in rows {
        let src = format!("{src}\nfn main() {{}}");
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(code, text)| (code, where_is(&src, text)))
            .collect();
        codes_row(row, &src, &expected);
    }
}

/// Operands of two types are the mistake in `==` and `!=`, whatever the bounds of the types: a
/// missing `Eq` is reported only for operands of one type.
#[test]
fn equality_should_report_operands_of_two_types_before_a_missing_operator() {
    let rows = [
        (
            "equality of a parameter and a number",
            "fn f<T>(a: T) -> bool { a == 5 }",
            vec![(Code::CannotCompare, "a == 5")],
        ),
        (
            "equality of a number and a parameter",
            "fn f<T>(a: T) -> bool { 5 == a }",
            vec![(Code::CannotCompare, "5 == a")],
        ),
        (
            "inequality of a parameter and a number",
            "fn f<T>(a: T) -> bool { a != 5 }",
            vec![(Code::CannotCompare, "a != 5")],
        ),
        (
            "equality of arrays of two types",
            "fn f<T>(a: [T], b: [i64]) -> bool { a == b }",
            vec![(Code::CannotCompare, "a == b")],
        ),
        (
            "equality of arrays of two parameters",
            "fn f<T, U>(a: [T], b: [U]) -> bool { a == b }",
            vec![(Code::CannotCompare, "a == b")],
        ),
    ];
    for (row, src, expected) in rows {
        let src = format!("{src}\nfn main() {{}}");
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(code, text)| (code, where_is(&src, text)))
            .collect();
        codes_row(row, &src, &expected);
    }
}

/// Ordered operands of two types are the mistake, whatever the bounds of the types: the missing
/// `<` is reported only for operands of one type, or for an operand whose own error leaves no type.
#[test]
fn comparisons_should_report_operands_of_two_types_before_a_missing_operator() {
    let rows = [
        (
            "order of two parameters that have Ord",
            "fn f<T: Ord, U: Ord>(a: T, b: U) -> bool { a < b }",
            vec![(Code::Mismatch, "b }")],
        ),
        (
            "at-least order of two parameters that have Ord",
            "fn f<T: Ord, U: Ord>(a: T, b: U) -> bool { a >= b }",
            vec![(Code::Mismatch, "b }")],
        ),
        (
            "a chain with a third operand of another parameter",
            "fn f<T: Ord, U: Ord>(a: T, b: T, c: U) -> bool { a < b < c }",
            vec![(Code::Mismatch, "c }")],
        ),
        (
            "order of a number and a parameter that has Ord",
            "fn f<U: Ord>(a: i64, b: U) -> bool { a < b }",
            vec![(Code::Mismatch, "b }")],
        ),
        (
            "order of a parameter and a number",
            "fn f<T>(a: T) -> bool { a < 5 }",
            vec![(Code::Mismatch, "5")],
        ),
        (
            "order of a number and a parameter",
            "fn f<T>(a: T) -> bool { 5 < a }",
            vec![(Code::Mismatch, "5")],
        ),
        (
            "order of two parameters that have no Ord",
            "fn f<T, U>(a: T, b: U) -> bool { a < b }",
            vec![(Code::Mismatch, "b }")],
        ),
        (
            "order of an enum with no Ord and a number",
            "enum L {\n    A\n}\nfn f() -> bool { L.A < 5 }",
            vec![(Code::Mismatch, "5")],
        ),
        (
            "order of a bool and a number",
            "fn f() -> bool { true < 5 }",
            vec![(Code::Mismatch, "5")],
        ),
        (
            "order of a class of a parameter and a number",
            "class Box<T> {\n    v: T\n}\nfn f<T>(a: Box<T>, n: i64) -> bool { a < n }",
            vec![(Code::Mismatch, "n }")],
        ),
        (
            "order of arrays of two parameters",
            "fn f<T, U>(a: [T], b: [U]) -> bool { a < b }",
            vec![(Code::Mismatch, "b }")],
        ),
        (
            "order of a string and a parameter",
            "fn f<T>(a: T) -> bool { \"x\" < a }",
            vec![(Code::Mismatch, "a }")],
        ),
        (
            "order of a parameter that has Ord and a parameter with none",
            "fn f<T: Ord, U>(a: T, b: U) -> bool { b < a }",
            vec![(Code::Mismatch, "a }")],
        ),
        (
            "a chain of a parameter and a number",
            "fn f<T>(a: T, b: T) -> bool { a < b < 5 }",
            vec![(Code::Mismatch, "5")],
        ),
    ];
    for (row, src, expected) in rows {
        let src = format!("{src}\nfn main() {{}}");
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(code, text)| (code, where_is(&src, text)))
            .collect();
        codes_row(row, &src, &expected);
    }
}

/// The controls of the two tests above for operands of two types: operands of one type, or an
/// operand with no type, still get the missing operator.
#[test]
fn comparisons_should_report_a_missing_operator_for_operands_of_one_type() {
    let rows = [
        (
            "equality of one parameter",
            "fn f<T>(a: T, b: T) -> bool { a == b }",
            vec![(Code::ParamLacks, "a == b")],
        ),
        (
            "order of one parameter",
            "fn f<T>(a: T, b: T) -> bool { a < b }",
            vec![(Code::ParamLacks, "a < b")],
        ),
        (
            "order of one class with no Ord",
            "class Node {}\nfn f(n: Node) -> bool { n < n }",
            vec![(Code::NoOperator, "n < n")],
        ),
        (
            "order of a parameter and an unknown name",
            "fn f<T>(a: T) -> bool { a < bad }",
            vec![(Code::ParamLacks, "a <"), (Code::BindingNotFound, "bad")],
        ),
        (
            "equality of a parameter and an unknown name",
            "fn f<T>(a: T) -> bool { a == bad }",
            vec![(Code::ParamLacks, "a =="), (Code::BindingNotFound, "bad")],
        ),
    ];
    for (row, src, expected) in rows {
        let src = format!("{src}\nfn main() {{}}");
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(code, text)| (code, where_is(&src, text)))
            .collect();
        codes_row(row, &src, &expected);
    }
}

/// `contains` is a method of the prelude's `extend<T: Eq> [T]`: an element with no `Eq` is a bound
/// that fails at the user's call, and no label points into the prelude. The words of each form
/// are pinned in the fixtures `E0154-contains-*`.
#[test]
fn contains_should_report_an_element_without_eq_as_not_conforming_at_the_call() {
    with_prelude();
    let node = "class Node {}\n";
    // Each row: the program, and the call that the error stands at.
    let rows = [
        (
            "a class",
            format!("{node}fn f(nodes: [Node], n: Node) -> bool {{ nodes.contains(n) }}"),
            "nodes.contains(n)",
        ),
        (
            "an optional class and `None`",
            format!("{node}fn f(xs: [Node?]) -> bool {{ xs.contains(None) }}"),
            "xs.contains(None)",
        ),
        (
            "a type parameter",
            "fn has<T>(xs: [T], x: T) -> bool { xs.contains(x) }".to_string(),
            "xs.contains(x)",
        ),
        (
            "an optional type parameter",
            "fn has<T>(xs: [T?], x: T?) -> bool { xs.contains(x) }".to_string(),
            "xs.contains(x)",
        ),
        (
            "an optional associated type",
            "interface Plain {\n    type Item\n}\nfn has<X: Plain>(xs: [X.Item?]) -> bool { xs.contains(None) }".to_string(),
            "xs.contains(None)",
        ),
        (
            "an array of arrays of an associated type",
            "interface Plain {\n    type Item\n}\nfn has<X: Plain>(xs: [[X.Item]], x: [X.Item]) -> bool { xs.contains(x) }".to_string(),
            "xs.contains(x)",
        ),
    ];
    for (row, src, call) in rows {
        let src = format!("{src}\nfn main() {{}}");
        let (line, col) = where_is(&src, call);
        diagnostic_row(row, &src, Code::NotConform, (line, col, call), &[]);
    }
}

#[test]
fn contains_should_report_an_argument_that_is_not_an_element_first() {
    with_prelude();
    // An argument of another type is the first mistake, as it is for `==`: a conformance to `Eq`
    // would not make the program right. Only an element and the contents of an optional element
    // are searched for.
    let node = "class Node {}\n";
    let rows = [
        (
            "a number for classes",
            format!("{node}fn f(xs: [Node]) -> bool {{ xs.contains(5) }}"),
            (Code::Mismatch, "5"),
        ),
        (
            "a number for optional classes",
            format!("{node}fn f(xs: [Node?]) -> bool {{ xs.contains(5) }}"),
            (Code::Mismatch, "5"),
        ),
        (
            "a class for optional classes",
            format!("{node}fn f(xs: [Node?], n: Node) -> bool {{ xs.contains(n) }}"),
            (Code::NotConform, "xs.contains(n)"),
        ),
        (
            "`None` for optional classes",
            format!("{node}fn f(xs: [Node?]) -> bool {{ xs.contains(None) }}"),
            (Code::NotConform, "xs.contains(None)"),
        ),
        (
            "`None` for classes",
            format!("{node}fn f(xs: [Node]) -> bool {{ xs.contains(None) }}"),
            (Code::Mismatch, "None"),
        ),
        (
            "`None` for enums that hold a class",
            format!(
                "{node}enum H {{\n    N(Node)\n}}\nfn f(xs: [H]) -> bool {{ xs.contains(None) }}"
            ),
            (Code::Mismatch, "None"),
        ),
        (
            "`None` for arrays of classes",
            format!("{node}fn f(xs: [[Node]]) -> bool {{ xs.contains(None) }}"),
            (Code::Mismatch, "None"),
        ),
        (
            "a number for a type parameter",
            "fn f<T>(xs: [T]) -> bool { xs.contains(5) }".to_string(),
            (Code::Mismatch, "5"),
        ),
        (
            "another type parameter for a type parameter",
            "fn f<T, U>(xs: [T], x: U) -> bool { xs.contains(x) }".to_string(),
            (Code::Mismatch, "x)"),
        ),
        (
            "an optional type parameter for a type parameter",
            "fn f<T>(xs: [T], x: T?) -> bool { xs.contains(x) }".to_string(),
            (Code::Mismatch, "x)"),
        ),
        (
            "`None` for a type parameter",
            "fn f<T>(xs: [T]) -> bool { xs.contains(None) }".to_string(),
            (Code::Mismatch, "None"),
        ),
        (
            "`None` in a block for classes",
            format!("{node}fn f(xs: [Node]) -> bool {{ xs.contains({{ None }}) }}"),
            (Code::Mismatch, "None"),
        ),
        (
            "`None` in a branch for classes",
            format!(
                "{node}fn f(xs: [Node], n: Node, c: bool) -> bool {{ xs.contains(if c {{ n }} else {{ None }}) }}"
            ),
            (Code::Mismatch, "None"),
        ),
        (
            "an empty array for classes",
            format!("{node}fn f(xs: [Node]) -> bool {{ xs.contains([]) }}"),
            (Code::Mismatch, "[]"),
        ),
        (
            "`None` in a block for optional classes",
            format!("{node}fn f(xs: [Node?]) -> bool {{ xs.contains({{ None }}) }}"),
            (Code::NotConform, "xs.contains({ None })"),
        ),
        (
            "an empty array for arrays of classes",
            format!("{node}fn f(xs: [[Node]]) -> bool {{ xs.contains([]) }}"),
            (Code::NotConform, "xs.contains([])"),
        ),
        (
            "a type parameter for optional type parameters",
            "fn f<T>(xs: [T?], x: T) -> bool { xs.contains(x) }".to_string(),
            (Code::NotConform, "xs.contains(x)"),
        ),
    ];
    for (row, src, (code, text)) in rows {
        let src = format!("{src}\nfn main() {{}}");
        codes_row(row, &src, &[(code, where_is(&src, text))]);
    }
}

/// A mismatch inside an operand is the only error: the operands are not one type, so the missing
/// operator is not reported.
#[test]
fn operators_should_report_only_a_mismatch_inside_an_operand() {
    with_prelude();
    let rows = [
        (
            "equality of arrays of two element types",
            "fn f() -> bool { [1] == [true] }",
            vec![(Code::Mismatch, "true")],
        ),
        (
            "inequality of arrays of two element types",
            "fn f() -> bool { [1] != [true] }",
            vec![(Code::Mismatch, "true")],
        ),
        (
            "order of arrays of two element types",
            "fn f() -> bool { [1] < [true] }",
            vec![(Code::Mismatch, "true")],
        ),
        (
            "order of an array of a parameter and an array of a number",
            "fn f<T>(a: [T]) -> bool { a < [5] }",
            vec![(Code::Mismatch, "5]")],
        ),
        (
            "order of nested arrays of two element types",
            "fn f() -> bool { [[1]] < [[true]] }",
            vec![(Code::Mismatch, "true")],
        ),
        (
            "a chain with an inner mismatch in the middle operand",
            "fn f<T>(a: [T]) -> bool { a < [5] < a }",
            vec![(Code::Mismatch, "5]")],
        ),
        (
            "a chain of three arrays with an inner mismatch in the middle operand",
            "fn f() -> bool { [1] < [true] < [2] }",
            vec![(Code::Mismatch, "true")],
        ),
        (
            "arrays of another element type for arrays of numbers",
            "fn f(xs: [[i64]]) -> bool { xs.contains([true]) }",
            vec![(Code::Mismatch, "true")],
        ),
        (
            "an array of a number for arrays of a parameter",
            "fn f<T>(xs: [[T]]) -> bool { xs.contains([5]) }",
            vec![(Code::Mismatch, "5]")],
        ),
        (
            "equality of an array of a parameter and an array of a number",
            "fn f<T>(a: [T]) -> bool { a == [true] }",
            vec![(Code::Mismatch, "true")],
        ),
        (
            "order with a branch of another type",
            "fn f<T>(a: [T], c: bool) -> bool { a < if c { true } else { a } }",
            vec![(Code::Mismatch, "true")],
        ),
        (
            "order with a branch that holds a number that does not fit",
            "fn f(a: [u8], c: bool) -> bool { a < if c { [300] } else { a } }",
            vec![(Code::IntegerDoesNotFit, "300")],
        ),
        (
            "order of bytes and a number that does not fit",
            "fn f(b: [u8]) -> bool { b < [300] }",
            vec![(Code::IntegerDoesNotFit, "300")],
        ),
        (
            "order of bytes and a negated number",
            "fn f(b: [u8]) -> bool { b < [-1] }",
            vec![(Code::NegatedU8, "-1")],
        ),
        (
            "order of arrays of bytes and a number that does not fit",
            "fn f(b: [[u8]]) -> bool { b <= [[256]] }",
            vec![(Code::IntegerDoesNotFit, "256")],
        ),
        (
            "a chain with a number that does not fit in the last operand",
            "fn f(b: [u8]) -> bool { b < b < [1, 300] }",
            vec![(Code::IntegerDoesNotFit, "300")],
        ),
        (
            "order of floats and an integer that a float cannot hold",
            "fn f(a: [f64]) -> bool { a < [9007199254740993] }",
            vec![(Code::NotExactlyF64, "9007199254740993")],
        ),
    ];
    for (row, src, expected) in rows {
        let src = format!("{src}\nfn main() {{}}");
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(code, text)| (code, where_is(&src, text)))
            .collect();
        codes_row(row, &src, &expected);
    }
}

/// A number that does not fit is a mismatch, as a value of another type is: the call that takes
/// it has no type, so its use as a `string` gives no second error.
#[test]
fn a_call_with_a_number_that_does_not_fit_should_report_only_that_number() {
    let src =
        "fn g(x: u8) -> u8 { x }\nfn f() -> bool { let s: string = g(300); true }\nfn main() {}";
    codes_row(
        "a call with a number that does not fit, bound to a string",
        src,
        &[(Code::IntegerDoesNotFit, where_is(src, "300"))],
    );
}

/// An error in an operand that is not about its type does not hide the missing operator: the
/// operands may still be one type.
#[test]
fn operators_should_report_a_missing_operator_beside_an_error_that_is_not_about_the_type() {
    with_prelude();
    let node = "class Node {}\n";
    let rows = [
        (
            "order of a class and a block with a wrong binding",
            format!("{node}fn f(a: Node) -> bool {{ a < {{ let x: i64 = true; a }} }}"),
            vec![(Code::NoOperator, "a <"), (Code::Mismatch, "true")],
        ),
        (
            "order of a parameter and a block with a wrong binding",
            "fn f<T>(a: T) -> bool { a < { let x: i64 = true; a } }".to_string(),
            vec![(Code::ParamLacks, "a <"), (Code::Mismatch, "true")],
        ),
        (
            "equality of a class and a block with a wrong binding",
            format!("{node}fn f(a: Node) -> bool {{ a == {{ let x: i64 = true; a }} }}"),
            vec![(Code::NoOperator, "a =="), (Code::Mismatch, "true")],
        ),
        (
            "order of a class and a call with a wrong argument",
            format!("{node}fn g(x: i64) -> i64 {{ x }}\nfn f(a: Node) -> bool {{ a < g(true) }}"),
            vec![(Code::NoOperator, "a <"), (Code::Mismatch, "true")],
        ),
        (
            "contains of a parameter and a block with a wrong binding",
            "fn f<T>(xs: [T], a: T) -> bool { xs.contains({ let x: i64 = true; a }) }".to_string(),
            vec![(Code::NotConform, "xs.contains("), (Code::Mismatch, "true")],
        ),
        (
            "contains of a class and a block with a wrong binding",
            format!(
                "{node}fn f(xs: [Node], a: Node) -> bool {{ xs.contains({{ let x: i64 = true; a }}) }}"
            ),
            vec![(Code::NotConform, "xs.contains("), (Code::Mismatch, "true")],
        ),
        (
            "contains of an optional parameter and a block with a wrong binding",
            "fn f<T>(xs: [T?], a: T) -> bool { xs.contains({ let x: i64 = true; a }) }".to_string(),
            vec![(Code::NotConform, "xs.contains("), (Code::Mismatch, "true")],
        ),
    ];
    for (row, src, expected) in rows {
        let src = format!("{src}\nfn main() {{}}");
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(code, text)| (code, where_is(&src, text)))
            .collect();
        codes_row(row, &src, &expected);
    }
}

/// A warning in an operand is not an error in it: the missing operator is still reported.
#[test]
fn operators_should_report_a_missing_operator_beside_a_warning_in_an_operand() {
    with_prelude();
    let node = "class Node {}\n";
    let rows = [
        (
            "contains of a parameter",
            "fn f<T>(xs: [T], x: T) -> bool { xs.contains({ let mut y = x; y }) }".to_string(),
            vec![
                (Code::NotConform, "xs.contains("),
                (Code::NeverReassigned, "y = x"),
            ],
        ),
        (
            "contains of a class",
            format!(
                "{node}fn f(xs: [Node], n: Node) -> bool {{ xs.contains({{ let mut y = n; y }}) }}"
            ),
            vec![
                (Code::NotConform, "xs.contains("),
                (Code::NeverReassigned, "y = n"),
            ],
        ),
        (
            "equality of a parameter",
            "fn f<T>(a: T, b: T) -> bool { a == { let mut y = b; y } }".to_string(),
            vec![(Code::ParamLacks, "a =="), (Code::NeverReassigned, "y = b")],
        ),
        (
            "order of a parameter",
            "fn f<T>(a: T, b: T) -> bool { a < { let mut y = b; y } }".to_string(),
            vec![(Code::ParamLacks, "a <"), (Code::NeverReassigned, "y = b")],
        ),
    ];
    for (row, src, expected) in rows {
        let src = format!("{src}\nfn main() {{}}");
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(code, text)| (code, where_is(&src, text)))
            .collect();
        codes_row(row, &src, &expected);
    }
}

#[test]
fn operators_should_check_without_eq_where_none_is_needed() {
    let rows = [
        (
            "identity of two objects",
            "class Node {}\nfn f(a: Node, b: Node) -> bool { a is b }",
        ),
        (
            "an optional class against `None`",
            "class Node {\n    next: Node?\n}\nfn f(n: Node) -> bool { n.next == None }",
        ),
        (
            "`None` on the left",
            "class Node {\n    next: Node?\n}\nfn f(n: Node) -> bool { None != n.next }",
        ),
        (
            "an optional array of classes against `None`",
            "class Node {}\nfn f(xs: [Node]?) -> bool { xs == None }",
        ),
    ];
    for (row, src) in rows {
        clean_row(row, &format!("{src}\nfn main() {{}}"));
    }
}

#[test]
fn a_comparison_chain_with_an_exiting_operand_should_bind_its_operands_first() {
    // The second operand leaves the function, so the first, a counted class, is read after it.
    let src = format!(
        "{CIRCLE}fn f(c: Circle, d: Circle?) -> bool {{\n    c < (if let x = d {{ x }} else {{ return true }})\n}}\nfn main() {{}}"
    );
    clean(&src);
}

#[test]
fn an_enum_with_an_unconditional_written_eq_should_take_only_the_hub_edge() {
    // The generated rule does not run for the `Wrap<[U]>` inside the array: its written `eq` does,
    // and it grows nothing.
    let src = "enum Wrap<T> {\n    One(T)\n}\nextend<T> Wrap<T>: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\n\
               fn same<U: Eq>(a: [Wrap<[U]>], b: [Wrap<[U]>]) -> bool {\n    a == b\n}\nfn main() {}";
    clean(src);
}

#[test]
fn a_cycle_through_the_generated_eq_of_an_enum_should_name_the_enum() {
    // `Pick<U>` holds a `Wrap<[U]>`, whose `eq` compares a `Pick<T>`: each turn of the cycle puts
    // two levels more into `U` and takes one off at the hub. The first step in the source is the
    // payload of `Pick`, so the error names the enum.
    let src = "class Wrap<T> {\n    x: T\n}\nenum Pick<U> {\n    One(Wrap<[U]>)\n}\n\
               extend<T: Eq> Wrap<T>: Eq {\n    fn eq(self, other: Self) -> bool {\n        Pick.One(Wrap(x: [self.x])) == Pick.One(Wrap(x: [other.x]))\n    }\n}\n\
               fn main() {\n    print(Wrap(x: 1) == Wrap(x: 2))\n}";
    codes(
        src,
        &[(Code::InstantiationCycle, where_is(src, "Wrap<[U]>"))],
    );
}

#[test]
fn an_operator_should_reach_a_written_method_through_its_hub_edge() {
    with_prelude();
    let w = "class W<T> {\n    x: T\n}\n";
    // Each row: the program, and the text of the call that closes an E0140, if any. In each, the
    // only way back from the callee to the method is the hub of the interface that its operator
    // reaches.
    let rows = [
        (
            "order on a parameter",
            format!(
                "{w}extend<T> W<T>: Eq {{\n    fn eq(self, other: Self) -> bool {{ true }}\n}}\nfn go<A: Ord>(a: A, b: A) -> bool {{\n    a < b\n}}\nextend<T> W<T>: Ord {{\n    fn less(self, other: Self) -> bool {{\n        go(W(x: [self.x]), W(x: [other.x]))\n    }}\n}}\nfn main() {{\n    print(W(x: 1) < W(x: 2))\n}}"
            ),
            Some("go(W(x: [self.x]), W(x: [other.x]))"),
        ),
        (
            "contains on an array of a parameter",
            format!(
                "{w}fn has<A: Eq>(xs: [A], x: A) -> bool {{\n    xs.contains(x)\n}}\nextend<T: Eq> W<T>: Eq {{\n    fn eq(self, other: Self) -> bool {{\n        has([W(x: [self.x])], W(x: [other.x]))\n    }}\n}}\nfn main() {{\n    print(W(x: 1) == W(x: 2))\n}}"
            ),
            Some("has([W(x: [self.x])], W(x: [other.x]))"),
        ),
        (
            "equality with `None` calls nothing",
            format!(
                "{w}fn go<A>(a: A?) -> bool {{\n    a == None\n}}\nextend<T: Eq> W<T>: Eq {{\n    fn eq(self, other: Self) -> bool {{\n        go(W(x: [self.x]))\n    }}\n}}\nfn main() {{\n    print(W(x: 1) == W(x: 2))\n}}"
            ),
            None,
        ),
    ];
    for (row, src, cycle) in rows {
        match cycle {
            Some(text) => codes_row(
                row,
                &src,
                &[(Code::InstantiationCycle, where_is(&src, text))],
            ),
            None => clean_row(row, &src),
        }
    }
}

#[test]
fn written_records_should_tell_a_conditional_conformance_from_an_unconditional_one() {
    let src = "enum Free<T> {\n    A(T)\n}\nextend<T> Free<T>: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\n\
               enum Bound<T> {\n    A(T)\n}\nextend<T: Ord> Bound<T>: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\n\
               enum Plain<T> {\n    A(T)\n}\n";
    let records = with_probe(src, ("T", "i64"), "Eq", |checker, _, interface| {
        let named = |name: &str| {
            let at = checker
                .enums
                .iter()
                .position(|e| e.name.name.as_str() == name);
            tir::EnumId(at.expect("the program should hold the enum"))
        };
        let records = checker.written_records();
        let of = |name| records.get(&(interface, named(name))).copied();
        (of("Free"), of("Bound"), of("Plain"))
    });
    assert_eq!(records, (Some((true, true)), Some((true, false)), None));
}

#[test]
fn eq_and_less_through_a_bound_should_stay_interface_calls() {
    // `mono` resolves them for each type argument: a type parameter's call is not the operator.
    let src = "fn same<T: Eq>(a: T, b: T) -> bool { a.eq(b) }\nfn below<T: Ord>(a: T, b: T) -> bool { a.less(b) }\nfn main() {\n    print(same(1, 2))\n    print(below(1, 2))\n}";
    contains(src, &["(call Eq.eq a#0 b#1)", "(call Ord.less a#0 b#1)"]);
}

const USER: &str = "class User {\n    name: string?\n}\n";
const FIND: &str = "fn find() -> string? {\n    None\n}\n";
const SHOW: &str = "fn show<T>(x: T) {\n    print(x)\n    print(\"{x}\")\n}\n";
const ITEM: &str = "enum Item {\n    Tag(string?)\n}\n";

/// A row of `PRINTED`: its name, the declarations, the parameters of `f`, the statement of the
/// body of `f`, and the text of the expression that gets the warning, if any.
type Row = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
);

const PRINTED: [Row; 35] = [
    ("print", "", "name: string?", "print(name)", Some("name")),
    ("eprint", "", "name: string?", "eprint(name)", Some("name")),
    (
        "interpolation",
        "",
        "name: string?",
        "print(\"{name}\")",
        Some("name"),
    ),
    (
        "interpolation with a width",
        "",
        "name: string?",
        "print(\"{name:>8}\")",
        Some("name"),
    ),
    (
        "interpolation with the pretty flag",
        "",
        "name: string?",
        "print(\"{name:#}\")",
        Some("name"),
    ),
    (
        "element of an array",
        "",
        "xs: [string?]",
        "print(xs[0])",
        Some("xs[0]"),
    ),
    (
        "field path",
        USER,
        "u: User",
        "print(u.name)",
        Some("u.name"),
    ),
    (
        "safe chain",
        USER,
        "u: User?",
        "print(u?.name)",
        Some("u?.name"),
    ),
    ("call result", FIND, "", "print(find())", Some("find()")),
    (
        "optional of an optional",
        "",
        "n: i64??",
        "print(n)",
        Some("n"),
    ),
    (
        "optional of an optional, interpolated",
        "",
        "n: i64??",
        "print(\"{n}\")",
        Some("n"),
    ),
    ("optional array", "", "xs: [i64]?", "print(xs)", Some("xs")),
    (
        "optional array, interpolated",
        "",
        "xs: [i64]?",
        "print(\"{xs}\")",
        Some("xs"),
    ),
    (
        "optional of a class",
        "class Node {}\n",
        "n: Node?",
        "print(n)",
        Some("n"),
    ),
    (
        "`??` that leaves an optional",
        "",
        "a: string??, b: string?",
        "print(a ?? b)",
        Some("a ?? b"),
    ),
    ("array of optionals", "", "xs: [string?]", "print(xs)", None),
    (
        "array literal of an optional",
        "",
        "name: string?",
        "print([name])",
        None,
    ),
    (
        "interpolated array of optionals",
        "",
        "xs: [string?]",
        "print(\"{xs}\")",
        None,
    ),
    (
        "payload of an enum",
        ITEM,
        "name: string?",
        "print(Item.Tag(name))",
        None,
    ),
    (
        "to_string",
        "",
        "name: string?",
        "print(name.to_string())",
        None,
    ),
    (
        "to_string of an optional of an optional",
        "",
        "n: i64??",
        "print(n.to_string())",
        None,
    ),
    (
        "interpolated to_string",
        "",
        "name: string?",
        "print(\"{name.to_string()}\")",
        None,
    ),
    (
        "default with `??`",
        "",
        "name: string?",
        "print(name ?? \"x\")",
        None,
    ),
    (
        "default with `??`, interpolated",
        "",
        "name: string?",
        "print(\"{name ?? \"x\"}\")",
        None,
    ),
    (
        "unwrapped with `if let`",
        "",
        "name: string?",
        "if let n = name {\n        print(n)\n    }",
        None,
    ),
    ("a string", "", "name: string", "print(name)", None),
    (
        "a type parameter",
        SHOW,
        "name: string?",
        "show(name)",
        None,
    ),
    (
        "a type parameter, array of optionals",
        SHOW,
        "xs: [string?]",
        "show(xs)",
        None,
    ),
    (
        "an optional that is not printed",
        "",
        "name: string?",
        "let copy = name",
        None,
    ),
    (
        "a comparison",
        "",
        "name: string?",
        "print(name == None)",
        None,
    ),
    ("a Some", "", "", "print(Some(1))", None),
    ("a Some in parentheses", "", "", "print((Some(1)))", None),
    ("Option.Some", "", "", "print(Option.Some(1))", None),
    (
        "a Some of an optional",
        "",
        "",
        "print(Some(Some(1)))",
        None,
    ),
    ("an interpolated Some", "", "", "print(\"{Some(1)}\")", None),
];

/// A top-level optional in `print`, `eprint` or an interpolation, whatever the expression that
/// gives it, warns once at that expression; a `Some(x)`, an optional inside another value, a text
/// from `to_string` or `??`, and a `T` that holds an optional when it is instantiated, do not. The
/// fixtures `W0015` and `W0015-value` pin the text.
#[test]
fn printing_an_optional_should_warn_only_at_the_top_level() {
    for (row, decls, params, stmt, warned) in PRINTED {
        let src = format!("{decls}fn f({params}) {{\n    {stmt}\n}}\nfn main() {{}}");
        match warned {
            Some(text) => {
                let call = stmt.find('(').expect("a row prints through a call") + 1;
                let col = 5 + call + stmt[call..].find(text).expect("the row holds the text");
                let line = decls.lines().count() + 2;
                diagnostic_row(row, &src, Code::PrintsOptional, (line, col, text), &[]);
            }
            None => clean_row(row, &src),
        }
    }
}

/// `print(None)` has no type for its `None`: the call gets no warning on top of the error.
#[test]
fn printing_none_should_not_infer_a_type() {
    codes(
        "fn main() {\n    print(None)\n}",
        &[(Code::CannotInfer, (2, 11))],
    );
}

/// `is` on a type parameter bound to `AnyObject`, directly or through an interface that
/// inherits it, and on its optional.
#[test]
fn is_should_check_on_an_any_object_parameter() {
    for (row, decls, signature) in [
        ("the bound", "", "same<T: AnyObject>(a: T, b: T)"),
        ("an optional", "", "same<T: AnyObject>(a: T?, b: T?)"),
        (
            "an object beside an optional",
            "",
            "same<T: AnyObject>(a: T, b: T?)",
        ),
        (
            "a bound that inherits it",
            "interface Observer: AnyObject {}\n",
            "same<T: Observer>(a: T, b: T)",
        ),
        (
            "a bound beside another",
            "interface Named {}\n",
            "same<T: Named & AnyObject>(a: T, b: T)",
        ),
    ] {
        let src = format!("{decls}fn {signature} -> bool {{\n    a is b\n}}\nfn main() {{}}");
        clean_row(row, &src);
    }
}

/// `is` on a type parameter that has the bound is still E0046 when its type is no object. An
/// array of `T` is no object whatever `T` is. `T??` can hold a `None` of `T?`, so one layer comes
/// off first.
#[test]
fn is_should_report_a_bound_parameter_that_is_no_object() {
    for (row, signature) in [
        (
            "an array of the parameter",
            "same<T: AnyObject>(a: [T], b: [T])",
        ),
        ("a double optional", "same<T: AnyObject>(a: T??, b: T??)"),
    ] {
        let src = format!("fn {signature} -> bool {{\n    a is b\n}}\nfn main() {{}}");
        codes_row(row, &src, &[(Code::IsNeedsObjects, (2, 5))]);
    }
}

/// `is` with two layers of `?` or more on a side reports once, at the left side, whichever side
/// has them. The fixtures `E0046-nested`, `E0046-nested-both` and `E0046-nested-triple` pin the
/// help that peels each side.
#[test]
fn is_should_report_once_for_sides_with_extra_layers() {
    for (row, signature) in [
        ("both sides", "same(a: Node??, b: Node??)"),
        ("one side", "same(a: Node??, b: Node?)"),
        ("the right side", "same(a: Node?, b: Node??)"),
        ("three layers", "same(a: Node???, b: Node?)"),
        ("a parameter", "same<T: AnyObject>(a: T??, b: T??)"),
        ("a side that is no object", "same(a: Node??, b: i64)"),
    ] {
        let src =
            format!("class Node {{}}\nfn {signature} -> bool {{\n    a is b\n}}\nfn main() {{}}");
        let col = if row == "the right side" { 10 } else { 5 };
        codes_row(row, &src, &[(Code::IsNeedsObjects, (3, col))]);
    }
}

/// A side whose type is unknown does not hide the report on the other side.
#[test]
fn is_should_report_a_side_with_extra_layers_beside_an_unknown_side() {
    let src =
        "class Node {}\nfn same(a: Node??, b: Missing) -> bool {\n    a is b\n}\nfn main() {}";
    codes(
        src,
        &[(Code::UnknownType, (2, 23)), (Code::IsNeedsObjects, (3, 5))],
    );
}

/// The interfaces whose associated types the rows of the next two tests test with `is`.
const ASSOC_OBJECTS: &str = "interface Observer: AnyObject {}
interface Objs {
    type Item: AnyObject
    fn first(self) -> Item
    fn maybe(self) -> Item?
    fn double(self) -> Item??
    fn all(self) -> [Item]
}
interface Seen {
    type Item: Observer
    fn first(self) -> Item
}
interface Deep {
    type Inner: Objs
    fn inner(self) -> Inner
}
interface Plain {
    type Item
    fn first(self) -> Item
    fn all(self) -> [Item]
}
interface Equal {
    type Item: Eq
    fn first(self) -> Item
}
";

/// `is` on an associated type, as on a type parameter, needs the bound `AnyObject` on the
/// associated type: written, inherited or on a nested one, and for its optional.
#[test]
fn is_should_check_on_an_associated_type_that_is_an_object() {
    for (row, bound, expr) in [
        ("the bound", "Objs", "a.first() is b.first()"),
        ("an optional", "Objs", "a.maybe() is b.maybe()"),
        (
            "an object beside an optional",
            "Objs",
            "a.first() is b.maybe()",
        ),
        ("a bound that inherits it", "Seen", "a.first() is b.first()"),
        (
            "a nested one",
            "Deep",
            "a.inner().first() is b.inner().first()",
        ),
    ] {
        let src = format!(
            "{ASSOC_OBJECTS}fn same<T: {bound}>(a: T, b: T) -> bool {{\n    {expr}\n}}\nfn main() {{}}"
        );
        clean_row(row, &src);
    }
}

/// `is` on an associated type with no bound `AnyObject`, on an array of one, and on a double
/// optional of one is still E0046.
#[test]
fn is_should_report_an_associated_type_that_is_no_object() {
    let line = ASSOC_OBJECTS.lines().count() + 2;
    for (row, bound, expr) in [
        ("no bound", "Plain", "a.first() is b.first()"),
        ("another bound", "Equal", "a.first() is b.first()"),
        ("an array of it", "Plain", "a.all() is b.all()"),
        ("an array of an object", "Objs", "a.all() is b.all()"),
        ("a double optional", "Objs", "a.double() is b.double()"),
    ] {
        let src = format!(
            "{ASSOC_OBJECTS}fn same<T: {bound}>(a: T, b: T) -> bool {{\n    {expr}\n}}\nfn main() {{}}"
        );
        codes_row(row, &src, &[(Code::IsNeedsObjects, (line, 5))]);
    }
}

/// A report that an associated type lacks a bound gives the help to add it, as for a type
/// parameter. The fixtures `E0046-assoc` and `E0139-assoc` pin the text.
#[test]
fn an_associated_type_with_no_bound_should_get_an_add_bound_help() {
    let line = ASSOC_OBJECTS.lines().count() + 3;
    for (row, bound, stmt, code, text) in [
        (
            "is",
            "Plain",
            "let _ = a.first() is b.first()",
            Code::IsNeedsObjects,
            "a.first()",
        ),
        (
            "is, beside another bound",
            "Equal",
            "let _ = a.first() is b.first()",
            Code::IsNeedsObjects,
            "a.first()",
        ),
        (
            "==",
            "Plain",
            "let _ = a.first() == b.first()",
            Code::ParamLacks,
            "a.first()",
        ),
        (
            "an argument",
            "Plain",
            "need_eq(a.first())",
            Code::NotConform,
            "a.first()",
        ),
    ] {
        let src = format!(
            "{ASSOC_OBJECTS}fn need_eq<U: Eq>(x: U) {{}}\nfn f<T: {bound}>(a: T, b: T) {{\n    {stmt}\n}}\nfn main() {{}}"
        );
        let col = 5 + stmt.find(text).expect("the row holds the text");
        // E0154 points at the bound of `need_eq` too.
        let bound: &[(usize, usize)] = if code == Code::NotConform {
            &[(line - 2, 15)]
        } else {
            &[]
        };
        let d = diagnostic_row(row, &src, code, (line, col, text), bound);
        assert!(d.help().is_some(), "row `{row}`");
    }
}

/// E0046 on a type that names a parameter has no help when no bound alone fixes it: `T` has no
/// `==`, and `T??` also needs a layer peeled. The fixture `E0046-param` pins the
/// help where a bound does fix it.
#[test]
fn is_should_give_no_help_when_no_bound_alone_fixes_the_type() {
    for (row, signature) in [
        ("an array of the parameter", "same<T>(a: [T], b: [T])"),
        ("an array of optionals", "same<T>(a: [T?], b: [T?])"),
        ("a double optional", "same<T>(a: T??, b: T??)"),
        ("a triple optional", "same<T>(a: T???, b: T???)"),
        (
            "a double optional with a bound",
            "same<T: Display>(a: T??, b: T??)",
        ),
    ] {
        let src = format!("fn {signature} -> bool {{\n    a is b\n}}\nfn main() {{}}");
        let d = diagnostic_row(row, &src, Code::IsNeedsObjects, (2, 5, "a"), &[]);
        assert!(d.help().is_none(), "row `{row}`");
    }
}

/// `is` on a type parameter with no `AnyObject` bound is E0046, and a call that gives the bound
/// something that is no class is `NotConform`. The fixture `E0046-param` pins the help.
#[test]
fn is_should_report_a_parameter_with_no_object_bound() {
    let at_a = (2, 5);
    for (row, signature, expected) in [
        ("no bound", "same<T>(a: T, b: T)", at_a),
        ("another bound", "same<T: Display>(a: T, b: T)", at_a),
        ("an optional with no bound", "same<T>(a: T?, b: T?)", at_a),
    ] {
        let src = format!("fn {signature} -> bool {{\n    a is b\n}}\nfn main() {{}}");
        codes_row(row, &src, &[(Code::IsNeedsObjects, expected)]);
    }
    for (row, decls, call) in [
        ("a number", "", "same(1, 2)"),
        (
            "an enum",
            "enum Dir {\n    North\n}\n",
            "same(Dir.North, Dir.North)",
        ),
    ] {
        let src = format!(
            "{decls}fn same<T: AnyObject>(a: T, b: T) -> bool {{\n    a is b\n}}\nfn main() {{\n    print({call})\n}}"
        );
        codes_row(row, &src, &[(Code::NotConform, argument(&src, call))]);
    }
}
