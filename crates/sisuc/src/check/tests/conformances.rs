//! The prelude's language interfaces (`Display`, `Eq`, `Ord`, `AnyObject`) and the conformances
//! that the compiler writes for the types: the rules in `Checker::conforms`.

use std::fmt::Write as _;

use super::*;
use crate::check::cycle;
use crate::check::interface::LangPath;

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
    assert_eq!(steps(&chain(40)), (40, vec![]));
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
            "a growing enum under the bound of a class",
            format!("class C<T: Eq> {{}}\n{nest}enum U {{\n    A(C<Nest<i64>>)\n}}\nfn main() {{}}"),
            vec![(Code::InstantiationCycle, (4, 10))],
        ),
    ] {
        codes_row(row, &src, &expected);
    }
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

/// A call of a generated `to_string` asks whether its class has a field of that name. A class
/// answers once, not once for each call: n fields and many calls read n fields, and n fields with
/// n calls read in proportion to n (ruling F7).
#[test]
fn generated_to_string_lookup_should_read_the_fields_of_a_class_once() {
    let reads = |fields: usize, calls: usize| {
        let mut field_lines = String::new();
        for i in 0..fields {
            writeln!(field_lines, "    f{i}: i64").expect("a string should take text");
        }
        let calls = "    let _ = x.to_string()\n".repeat(calls);
        let src = format!("class C {{\n{field_lines}}}\nfn f(x: C) {{\n{calls}}}\nfn main() {{}}");
        DISPLAY_FIELD_READS.set(0);
        assert!(checked(&src).0.is_some(), "{src}");
        DISPLAY_FIELD_READS.get()
    };
    assert_eq!(reads(10, 1), 10, "one call should read each field once");
    assert_eq!(
        reads(10, 50),
        10,
        "fifty calls should read each field once, not fifty times"
    );
    assert_eq!(reads(0, 5), 0, "a class with no field has nothing to read");
    assert_near_linear(reads(100, 100), reads(200, 200), reads(300, 300));
}

/// A list of type arguments after `to_string` is the error of a method that takes none, whatever
/// the receiver: the generated `Display` takes none either, and a list that names a real type is
/// no better than one that names none. The rows with a written `to_string` are the control: they
/// have the same code at the same place.
#[test]
fn to_string_with_type_arguments_should_report_the_list() {
    let decls = "class C {}\nclass W {}\nextend W {\n    fn to_string(self) -> string { \"w\" }\n}\nclass B {}\nextend B: Display {\n    fn to_string(self) -> string { \"b\" }\n}\nenum E {\n    A\n}\n";
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
    ] {
        let generic = if param == "T" { "<T>" } else { "" };
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
/// error of its own: the part `{m}` of the receiver is an optional, which is the one error.
#[test]
fn to_string_on_a_receiver_that_failed_should_report_only_the_receiver() {
    codes(
        "fn f(m: i64?) {\n    print(\"{m}\".to_string())\n}\nfn main() {}",
        &[(Code::CannotInterpolate, (2, 13))],
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

/// A class with a written `Eq` and `Ord`, which the operator rows compare.
const CIRCLE: &str = "class Circle: Eq, Ord {\n    r: i64\n    fn eq(self, other: Self) -> bool { self.r == other.r }\n    fn less(self, other: Self) -> bool { self.r < other.r }\n}\n";

#[test]
fn operators_should_check_through_eq_and_ord() {
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
    let decls = "class Node {}\nclass Box<T> {\n    v: T\n}\nenum Wrap<T> {\n    One(T)\n}\n";
    // Each row: the function after the declarations, the text where the error stands, and the
    // code. The help text of each is pinned in the fixtures `E0139-*`.
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
            Code::ParamLacks,
        ),
        (
            "order of two parameters",
            "fn f<T, U>(a: T, b: U) -> bool { a < b < a }",
            "a < b",
            Code::ParamLacks,
        ),
        (
            "contains of another parameter",
            "fn f<T, U>(a: [T], b: U) -> bool { a.contains(b) }",
            "a.contains(b)",
            Code::ParamLacks,
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
            Code::ParamLacks,
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
            Some(growing),
        ),
        (
            "an interpolation of an enum payload, reached through its generated Display",
            format!(
                "{w}enum Pick<U> {{\n    One(U)\n}}\nextend<T> W<T>: Display {{\n    fn to_string(self) -> string {{\n        \"{{Pick.One(W(x: [self.x]))}}\"\n    }}\n}}\n{main}"
            ),
            Some("\"{Pick.One(W(x: [self.x]))}\""),
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
            "contains of a class",
            format!("{node}fn f(nodes: [Node], n: Node) -> bool {{ nodes.contains(n) }}"),
            "nodes.contains(n)",
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

#[test]
fn contains_should_report_an_argument_that_is_not_an_element_first() {
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
            (Code::NoOperator, "xs.contains(n)"),
        ),
        (
            "`None` for optional classes",
            format!("{node}fn f(xs: [Node?]) -> bool {{ xs.contains(None) }}"),
            (Code::NoOperator, "xs.contains(None)"),
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
            (Code::ParamLacks, "xs.contains(5)"),
        ),
    ];
    for (row, src, (code, text)) in rows {
        let src = format!("{src}\nfn main() {{}}");
        codes_row(row, &src, &[(code, where_is(&src, text))]);
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
