//! Generic declarations: the scopes of type parameters, type arguments in types and
//! expressions, what a value of a type parameter may do, and which generic enums can be built.

use super::*;

/// The classes the rows name, and an empty `main`, after the line a row tests.
const DECLS: &str = "class Cell<T> {\n    value: T\n    next: Cell<T>?\n}\nclass Stack<T> {\n    mut top: Cell<T>?\n}\nclass Point {\n    x: i64\n}\nfn main() {}";

/// `line` on line 1, then `DECLS`.
pub(super) fn with_decls(line: &str) -> String {
    format!("{line}\n{DECLS}")
}

#[test]
fn generic_declarations_should_check_clean() {
    for src in [
        "fn id<T>(x: T) -> T { x }",
        "fn wrap<T>(x: T) -> [T] { [x] }",
        "fn first<T>(xs: [T]) -> T? { if xs.len() == 0 { return None }\n xs[0] }",
        "fn none<T>(x: T?) -> bool { x == None }",
        "fn none<T>(x: T?) -> bool { x == Option.None }",
        "fn none<T>(x: T?) -> bool { Option.None == x }",
        "fn none<T>(x: T?) -> bool { x == Option<T>.None }",
        "fn none<T>(x: T?) -> bool { Option<T>.None != x }",
        "enum Wrap<T> { W(T) }",
        "class Queue<T> {\n    mut front: Cell<T>?\n    fn peek(self) -> Cell<T>? { self.front }\n    fn empty(self) -> bool { self.front == None }\n}",
        "enum Tree<T> {\n    Leaf\n    Node(Tree<T>, T, Tree<T>)\n}",
        "class Holder<T> {\n    value: T\n    fn get(self) -> T { self.value }\n    fn pair<U>(self, u: U) -> [U] { [u] }\n}",
        "fn id<Point>(x: Point) -> Point { x }",
        "fn f(s: Stack<i64>, t: [Stack<[i64]>]) {}",
        "fn f(s: Stack<[i64?]>) {}",
        "fn f(s: Stack<i64>) -> Cell<i64>? { s.top }",
        // A field is assigned the type the receiver's arguments make it, as it is read.
        "class Box<T> {\n    mut v: T\n}\nfn f<U>(b: Box<U>, u: U) { b.v = u }",
        "class Box<T> {\n    mut v: T\n}\nfn f(b: Box<i64>) { b.v = 1 }",
        "class Box<T> {\n    mut v: T\n}\nfn f(b: Box<i64>) { b.v += 1 }",
        "class Box<T> {\n    mut v: T\n}\nfn f(b: Box<[i64]>) { b.v[0] += 1 }",
        "fn f<T>(n: i64) -> [T?] { [None; n] }",
        "fn g<T>(xs: [T]) -> [[T]] { [xs; 2] }",
        "fn g(a: i64, b: i64, c: i64, d: i64) { f((a < b), c > (d)) }\nfn f(x: bool, y: bool) {}",
        // A qualified variant pattern names the enum, whatever its arguments.
        "enum Tree<T> {\n    Leaf\n    Node(Tree<T>, T, Tree<T>)\n}\nfn count<T>(t: Tree<T>) -> i64 {\n    match t {\n        Tree.Leaf => 0\n        Tree.Node(_, _, _) => 1\n    }\n}",
        "enum Opt<T> {\n    No\n    Yes(T)\n    fn none(self) -> bool {\n        match self {\n            Opt.No => true\n            Opt.Yes(_) => false\n        }\n    }\n}",
    ] {
        clean(&with_decls(src));
    }
}

#[test]
fn type_parameter_lists_should_check_clean_over_many_lines() {
    for src in [
        "fn f<\n    T\n>(x: T) -> T {\n    x\n}",
        "class C<\n    T,\n    U\n> {\n    a: T\n    b: U\n}",
        "enum E<\n    T\n> {\n    A(T)\n}",
    ] {
        clean(&with_decls(src));
    }
}

/// `tir` names each declaration's type parameters, a method's owner's before its own.
#[test]
fn type_parameters_should_reach_tir() {
    contains(
        &with_decls(
            "class Holder<T> {\n    value: T\n    fn pair<U>(self, u: U) -> [U] { [u] }\n}\nenum Opt<T> {\n    No\n    Yes(T)\n    fn pick<U>(self, u: U) -> U { u }\n}",
        ),
        &[
            "(class Holder<T> (value T))",
            "(class Stack<T> (mut top Cell<T>?))",
            "(enum Opt<T> No (Yes T))",
            "(fn Holder<T>.pair<U> ((self#0 Holder<T>) (u#1 U)) [U] ",
            "(fn Opt<T>.pick<U> ((self#0 Opt<T>) (u#1 U)) U ",
        ],
    );
}

/// A value whose type names a parameter may be counted, so an exit after it spills it, as
/// does a generic enum, counted for its parameter.
#[test]
fn values_of_parameter_types_should_spill() {
    contains(
        "fn g<T>(x: T, c: bool) {\n    g(x, if c { return } else { false })\n}\nfn main() {}",
        &["(let #2 x#0)"],
    );
    contains(
        "enum Wrap<T> { W(T) }\nfn k(w: Wrap<string>, n: i64) {}\nfn h(w: Wrap<string>, c: bool) {\n    k(w, if c { return } else { 0 })\n}\nfn main() {}",
        &["(let #2 w#0)"],
    );
}

#[test]
fn type_arguments_that_do_not_fit_a_type_should_be_reported() {
    let arguments = |at| vec![(Code::TypeArguments, at)];
    let argument = vec![(Code::TypeArgument, (1, 15))];
    for (line, expected) in [
        ("fn f(s: Stack) {}", arguments((1, 9))),
        ("fn f(s: Stack<i64, i64>) {}", arguments((1, 14))),
        ("fn f(p: Point<i64>) {}", arguments((1, 14))),
        ("fn f(s: Stack<unit>) {}", argument.clone()),
        ("fn f(s: Stack<never>) {}", argument),
        ("fn f(s: Stack<Zz>) {}", vec![(Code::UnknownType, (1, 15))]),
        ("fn f(w: Wrap) {}\nenum Wrap<T> { W(T) }", arguments((1, 9))),
        (
            "fn f(p: Pair<i64>) {}\nclass Pair<A, B> { a: A }",
            arguments((1, 13)),
        ),
        (
            "fn f(p: Pair) {}\nclass Pair<A, B> { a: A }",
            arguments((1, 9)),
        ),
    ] {
        codes(&with_decls(line), &expected);
    }
    // The missing list is reported at the name, without its `?`.
    diagnostic(
        &with_decls("fn f(s: Stack?) {}"),
        Code::TypeArguments,
        (1, 9, "Stack"),
        &[],
    );
}

/// A non-generic name's list is deleted, in a type and in an expression alike.
#[test]
fn type_arguments_on_a_non_generic_name_should_be_removed() {
    for (line, at) in [
        ("fn f(p: Point<i64>) {}", (1, 14)),
        ("fn g() -> Point { Point<i64>(x: 1) }", (1, 24)),
    ] {
        let src = with_decls(line);
        let d = diagnostic(&src, Code::TypeArguments, (at.0, at.1, "<i64>"), &[]);
        assert_eq!(
            fixes(&src, &d),
            [(true, vec![(at.0, at.1, "<i64>", String::new())])]
        );
        fix_removes_its_code(&src, &d);
    }
}

/// `a<b, c>(d)` calls `a`, which is a value: one diagnostic, at the value, and the arguments
/// are not resolved.
#[test]
fn type_arguments_on_a_value_should_report_only_the_value() {
    let src = "fn g(a: i64, b: i64, c: i64, d: i64) { f(a < b, c > (d)) }\nfn f(x: bool) {}\nfn main() {}";
    let d = diagnostic(src, Code::TypeArguments, (1, 42, "a"), &[]);
    assert!(d.help().is_some());
    assert_eq!(d.fixes(), []);
    let src = with_decls("fn g(p: Point, a: i64, b: i64, c: i64) -> bool { p.x < a, b > (c) }");
    diagnostic(&src, Code::TypeArguments, (1, 50, "p.x"), &[]);
    // Before a `.`, the list cannot be two comparisons, so there is nothing to compare.
    for (line, at) in [
        ("fn g(xs: [i64]) -> i64 { xs<i64>.len() }", (1, 26, "xs")),
        ("fn g(p: Point) -> i64 { p<i64>.x }", (1, 25, "p")),
    ] {
        let d = diagnostic(&with_decls(line), Code::TypeArguments, at, &[]);
        assert!(d.help().is_none(), "{line}");
    }
    // Nor when the first argument is a type that cannot be a value.
    for (line, at) in [
        ("fn g(p: Point) { p.x<i64>(1) }", (1, 18, "p.x")),
        ("fn g(a: i64) { a<[i64]>(1) }", (1, 16, "a")),
        ("fn g(a: i64) { a<i64?>(1) }", (1, 16, "a")),
        ("fn g(a: i64) { a<Point<i64>>(1) }", (1, 16, "a")),
        // A class, an enum or a type parameter names a type too.
        ("fn g(p: Point) { p.x<Point>(1) }", (1, 18, "p.x")),
        (
            "fn g(p: Point) { p.x<Color>(1) }\nenum Color { Red }",
            (1, 18, "p.x"),
        ),
        ("fn g<T>(p: Point) { p.x<T>(1) }", (1, 21, "p.x")),
        (
            "fn g(p: Point) { U.A<Point>(p) }\nenum U { A }",
            (1, 18, "U.A"),
        ),
    ] {
        let d = diagnostic(&with_decls(line), Code::TypeArguments, at, &[]);
        assert!(d.help().is_none(), "{line}");
    }
    // A local of that name is a value.
    let line = "fn g(p: Point, Point: i64) { p.x<Point>(1) }";
    let d = diagnostic(&with_decls(line), Code::TypeArguments, (1, 30, "p.x"), &[]);
    assert!(d.help().is_some());
    // A built-in's method is compared as its result: the help calls one that takes no
    // arguments, and the call it writes checks. One that takes arguments has no help.
    let both = "fn both(x: bool, y: bool) -> bool { x && y }\nfn main() {}";
    let src = format!("fn g(a: [i64], b: i64) -> bool {{ both(a.len < b, b > (1)) }}\n{both}");
    let d = diagnostic(&src, Code::TypeArguments, (1, 39, "a.len"), &[]);
    assert!(d.help().is_some());
    clean(&format!(
        "fn g(a: [i64], b: i64) -> bool {{ both((a.len() < b), b > (1)) }}\n{both}"
    ));
    let src = format!("fn g(a: [i64], b: i64) -> bool {{ both(a.contains < b, b > (1)) }}\n{both}");
    let d = diagnostic(&src, Code::TypeArguments, (1, 39, "a.contains"), &[]);
    assert!(d.help().is_none());
    // A local that takes the name `math` is a value, whose field is compared.
    codes(
        &with_decls(
            "fn g(p: Point, a: i64, b: i64, c: i64) -> bool {\n    let math = p\n    math.x < a, b > (c)\n}",
        ),
        &[(Code::TypeArguments, (3, 5))],
    );
}

/// A pattern's qualifier that names a type other than an enum fits no enum.
#[test]
fn a_class_qualifier_should_fit_no_enum() {
    codes(
        "class K {\n    x: i64\n}\nenum E { A }\nfn f(e: E) -> i64 {\n    match e {\n        K.A => 1\n        _ => 0\n    }\n}\nfn main() {}",
        &[(Code::Mismatch, (7, 9))],
    );
}

/// The declarations the misread-comparison tests call.
const MISREAD_DECLS: &str = "enum E { A(bool, bool) }\nclass K {\n    fn m(self, x: bool, y: bool) {}\n}\nclass C {\n    x: bool\n    y: bool\n}\nfn g(x: i64) -> i64 { x }\nfn h(x: bool, y: bool) -> bool { x }\nfn p(x: bool, y: bool) {}\nfn main() {}";

/// Any `a<b, c>(d)` that does not check hides the count, whatever `a` is: a member of a type
/// that is a value, and a built-in's method, are compared, and an unknown name is reported
/// alone. One that checks, or a list without a comma, does not.
#[test]
fn a_failed_list_with_a_comma_should_hide_the_count() {
    let decls = MISREAD_DECLS;
    for (line, expected) in [
        (
            "fn f(x: f64) -> bool { h(f64.pi < x, x > (1.0)) }",
            vec![(Code::TypeArguments, (1, 26))],
        ),
        (
            "fn f(b: i64) -> bool { h(U.A < b, b > (1)) }\nenum U { A }",
            vec![(Code::TypeArguments, (1, 26))],
        ),
        // A string's method and a number's are compared, as an array's is.
        (
            "fn f(s: string, b: i64) -> bool { h(s.len < b, b > (1)) }",
            vec![(Code::TypeArguments, (1, 37))],
        ),
        (
            "fn f(x: f64, b: f64) -> bool { h(x.sqrt < b, b > (1.0)) }",
            vec![(Code::TypeArguments, (1, 34))],
        ),
        // A built-in's member that is no method is reported unknown, whatever follows it.
        (
            "fn f(a: [i64], b: i64) -> bool { h(a.nope < b, b > (1)) }",
            vec![(Code::NoMember, (1, 38))],
        ),
        (
            "fn f(s: string, b: i64) -> bool { h(s.nope < b, b > (1)) }",
            vec![(Code::NoMember, (1, 39))],
        ),
        // A list without a comma after a built-in's method, or one after another receiver's
        // method, takes no type arguments.
        (
            "fn f(a: [i64], b: i64) -> i64 { a.len<b>() }",
            vec![(Code::TypeArguments, (1, 38))],
        ),
        (
            "fn f(k: K, a: bool) { k.m<C, C>(a, a) }",
            vec![(Code::TypeArguments, (1, 26))],
        ),
        (
            "fn f(b: i64) -> bool { h(undefined < b, b > (1)) }",
            vec![(Code::FunctionNotFound, (1, 26))],
        ),
        (
            "fn f(a: [i64], b: i64) -> bool { h(a.len < b, b > (1)) }",
            vec![(Code::TypeArguments, (1, 36))],
        ),
        (
            "fn f() -> bool { h(undefined<i64>(1)) }",
            vec![
                (Code::ArgumentCount, (1, 18)),
                (Code::FunctionNotFound, (1, 20)),
            ],
        ),
        (
            "fn f() -> bool { h(two<i64, bool>(1, true)) }\nfn two<T, U>(t: T, u: U) -> bool { true }",
            vec![(Code::ArgumentCount, (1, 18))],
        ),
    ] {
        codes(&format!("{line}\n{decls}"), &expected);
    }
}

/// Lists such as `<i64, bool>` cannot be comparisons. Their inner errors do not hide the errors
/// of the outer call. A list whose ends name no type could be comparisons, and it keeps the
/// outer errors hidden.
#[test]
fn accepted_type_arguments_should_keep_outer_errors() {
    let decls = "fn h(a: bool, b: bool) -> bool { a }\nfn two<T, U>(t: T, u: U) -> bool { true }\nfn three<T, U, V>(t: T, u: U, v: V) -> bool { true }\nclass C {\n    v: i64\n}\nclass Box<T> {\n    v: T\n    fn two<U, V>(self, u: U, v: V) -> bool { true }\n}\nfn main() {}";
    for (line, expected) in [
        (
            "fn f() { print(h(two<i64, bool>(1, \"x\"))) }",
            vec![(Code::ArgumentCount, (1, 16)), (Code::Mismatch, (1, 36))],
        ),
        (
            "fn f() { print(h(two<i64, bool>(1))) }",
            vec![
                (Code::ArgumentCount, (1, 16)),
                (Code::ArgumentCount, (1, 18)),
            ],
        ),
        (
            "fn f(b: Box<i64>) { print(h(b.two<i64, bool>(1, \"x\"))) }",
            vec![(Code::ArgumentCount, (1, 27)), (Code::Mismatch, (1, 49))],
        ),
        (
            "fn f(b: Box<i64>) { print(h(b.two<i64, bool>(1))) }",
            vec![
                (Code::ArgumentCount, (1, 27)),
                (Code::ArgumentCount, (1, 29)),
            ],
        ),
        // An array type is no name, so it cannot join a comparison.
        (
            "fn f() { h(two<[i64], [bool]>([1])) }",
            vec![
                (Code::ArgumentCount, (1, 10)),
                (Code::ArgumentCount, (1, 12)),
            ],
        ),
        // A name that is a class names a type, even when a local takes its spelling.
        (
            "fn f() {\n    let C = 0\n    h(two<C, C>(1, C))\n}",
            vec![
                (Code::ArgumentCount, (3, 5)),
                (Code::Mismatch, (3, 17)),
                (Code::Mismatch, (3, 20)),
            ],
        ),
        (
            "fn f() {\n    let C = 0\n    h(three<C, bool, C>(1, true, C))\n}",
            vec![
                (Code::ArgumentCount, (3, 5)),
                (Code::Mismatch, (3, 25)),
                (Code::Mismatch, (3, 34)),
            ],
        ),
    ] {
        codes(&format!("{line}\n{decls}"), &expected);
    }
}

/// Comparisons misread as a call take the commas of the argument list they sit in, which then
/// has its count of arguments not reported: a function's, a variant's, a method's or a class's,
/// and wherever operators put the comparison in its argument. A list around that one, or around
/// a block that holds the comparison, still has its count reported.
#[test]
fn a_misread_comparison_should_hide_only_the_count_of_its_own_list() {
    let decls = MISREAD_DECLS;
    let misread = |col| (Code::TypeArguments, (1, col));
    let count = (Code::ArgumentCount, (1, 40));
    for (line, expected) in [
        (
            "fn f(a: i64, b: i64, c: i64, d: i64) -> E { E.A(a < b, c > (d)) }",
            vec![misread(49)],
        ),
        (
            "fn f(k: K, a: i64, b: i64, c: i64, d: i64) { k.m(a < b, c > (d)) }",
            vec![misread(50)],
        ),
        (
            "fn f(a: i64, b: i64, c: i64, d: i64) { g(1, { h(a < b, c > (d))\n 2 }) }",
            vec![count, misread(49)],
        ),
        (
            "fn f(a: i64, b: i64, c: i64, d: i64) { g(1, 2, h(a < b, c > (d))) }",
            vec![count, misread(50)],
        ),
    ] {
        codes(&format!("{line}\n{decls}"), &expected);
    }
    // Each row and the row its help corrects, which checks clean.
    let head = "fn f(a: i64, b: i64, c: i64, d: i64, x: bool?) -> C {\n    ";
    let body = |call: &str| {
        // A call of `p` is a statement before the `C` the function returns.
        let tail = if call.starts_with('p') {
            "\n    C(x: true, y: true)"
        } else {
            ""
        };
        format!("{head}{call}{tail}\n}}\n{decls}")
    };
    for (misread, corrected) in [
        ("p(true && a < b, c > (d))", "p(true && (a < b), c > (d))"),
        ("p(1 + a < b, c > (d))", "p((1 + a < b), c > (d))"),
        ("p(-a < b, c > (d))", "p((-a < b), c > (d))"),
        (
            "p(!(a == 1) == a < b, c > (d))",
            "p(!(a == 1) == (a < b), c > (d))",
        ),
        ("p(x ?? a < b, c > (d))", "p(x ?? (a < b), c > (d))"),
        ("p(0 < a < b, c > (d))", "p((0 < a < b), c > (d))"),
        (
            "p(1 + a < b, c > (d) && true)",
            "p((1 + a < b), c > (d) && true)",
        ),
        ("C(x: a < b, c > (d))", "C(x: (a < b), y: c > (d))"),
    ] {
        let col = misread
            .find("a < b")
            .expect("the row should compare `a < b`")
            + 5;
        codes(&body(misread), &[(Code::TypeArguments, (2, col))]);
        clean(&body(corrected));
    }
}

#[test]
fn values_of_parameter_types_should_have_no_operators() {
    let lacks = |at| vec![(Code::ParamLacks, at)];
    for (src, expected) in [
        ("fn f<T>(a: T, b: T) -> bool { a < b }", lacks((1, 31))),
        ("fn f<T>(a: T, b: T) -> bool { a == b }", lacks((1, 31))),
        ("fn f<T>(a: T, b: T) -> bool { a != b }", lacks((1, 31))),
        ("fn f<T>(a: T, b: T) -> bool { a + b }", lacks((1, 31))),
        ("fn f<T>(a: T, b: T) -> bool { -a }", lacks((1, 32))),
        ("fn f<T>(a: T, b: T) -> bool { [a] == [b] }", lacks((1, 31))),
        ("fn f<T>(a: T, b: i64) -> bool { b < a }", lacks((1, 37))),
        (
            "fn f<T>(xs: [T], a: T) -> bool { xs.contains(a) }",
            lacks((1, 34)),
        ),
        (
            "fn f<T>(a: T, b: T) -> [T] { [a; 2] }",
            vec![(Code::SameObject, (1, 31))],
        ),
        (
            "fn f<T>(a: T, b: T) { print(a) }",
            vec![(Code::PrintArgument, (1, 29))],
        ),
        (
            "fn f<T>(a: T, b: T) -> string { \"{a}\" }",
            vec![(Code::CannotInterpolate, (1, 35))],
        ),
        (
            "fn f<T>(a: T, b: T) -> i64 { a.len() }",
            vec![(Code::NoMember, (1, 32))],
        ),
        (
            "fn f<T>(a: T, b: T) -> i64 { a.x }",
            vec![(Code::NoMember, (1, 32))],
        ),
        (
            "fn f<T>(a: T, b: T) -> bool { a is b }",
            vec![(Code::IsNeedsObjects, (1, 31))],
        ),
        // Only the comparison is reported: `b` is still a `bool`.
        (
            "fn f<T>(a: T) -> T {\n    let b = a < a\n    if b { return a }\n    a\n}",
            lacks((2, 13)),
        ),
        // A type parameter is found before a class of its name.
        (
            "fn f<Point>(x: Point) -> i64 { x.x }",
            vec![(Code::NoMember, (1, 34))],
        ),
    ] {
        codes(&with_decls(src), &expected);
    }
}

/// The `==` help of `is` would point at an operator a parameter lacks.
#[test]
fn is_on_a_parameter_should_have_no_help() {
    let src = with_decls("fn f<T>(a: T, b: T) -> bool { a is b }");
    let d = diagnostic(&src, Code::IsNeedsObjects, (1, 31, "a"), &[]);
    assert!(d.help().is_none());
}

#[test]
fn main_should_take_no_type_parameters() {
    let src = "fn main<T>() {}";
    let d = diagnostic(src, Code::MainSignature, (1, 8, "<T>"), &[]);
    assert_eq!(fixes(src, &d), [(true, vec![(1, 8, "<T>", String::new())])]);
    fix_removes_its_code(src, &d);
    // The fix would leave `T` naming nothing.
    let src = "fn main<T>() { let x: T? = None }";
    let d = diagnostic(src, Code::MainSignature, (1, 8, "<T>"), &[]);
    assert_eq!(d.fixes(), []);
    // A string or a comment that holds the name does not use it.
    let src = "fn main<T>() {\n    print(\"T\") // T\n}";
    let d = diagnostic(src, Code::MainSignature, (1, 8, "<T>"), &[]);
    assert_eq!(fixes(src, &d), [(true, vec![(1, 8, "<T>", String::new())])]);
}

#[test]
fn type_parameter_names_should_not_clash() {
    for (src, expected) in [
        ("fn f<i64>() {}", vec![(Code::BuiltInName, (1, 6))]),
        ("fn f<T, T>() {}", vec![(Code::DefinedTwice, (1, 9))]),
        (
            "class Holder<T> {\n    value: T\n    fn get<T>(self) -> T { self.value }\n}",
            vec![(Code::DefinedTwice, (3, 12))],
        ),
        // A reported parameter binds to nothing at a call, and is not reported again there.
        (
            "fn f<T, T>(x: T) -> T { x }\nfn g() {\n    print(f(1))\n}",
            vec![(Code::DefinedTwice, (1, 9))],
        ),
        (
            "fn f<i64, T>(x: T) -> T { x }\nfn g() {\n    print(f(1))\n}",
            vec![(Code::BuiltInName, (1, 6))],
        ),
        (
            "class Box<T> {\n    v: T\n    fn m<T>(self, x: T) -> T { x }\n}\nfn g(b: Box<i64>) {\n    print(b.m(2))\n}",
            vec![(Code::DefinedTwice, (3, 10))],
        ),
        (
            "class Box<T, T> {\n    v: T\n}\nfn g() {\n    print(Box(v: 1).v)\n}",
            vec![(Code::DefinedTwice, (1, 14))],
        ),
        (
            "enum E<T, T> {\n    A(T)\n}\nfn g() {\n    let e = E.A(1)\n}",
            vec![(Code::DefinedTwice, (1, 11))],
        ),
        // One that is not reported still is.
        (
            "fn f<i64, T>() -> [T] { [] }\nfn g() {\n    print(f().len())\n}",
            vec![(Code::BuiltInName, (1, 6)), (Code::CannotInfer, (3, 11))],
        ),
    ] {
        codes(&format!("{src}\nfn main() {{}}"), &expected);
    }
    // A class's parameters are out of scope after it, here with no function after the class to
    // reset the scope.
    codes(
        "fn main() {}\nclass C<T> {\n    x: T\n}\nconst N: T = 1",
        &[(Code::UnknownType, (5, 10))],
    );
}

/// Which generic enums can be built depends on which of their arguments can: `Two<Expr>` only
/// if `Expr` can, and `Nest<T>` needs a `Nest<[T]>`, which needs what it does.
#[test]
fn generic_enums_should_be_built_only_from_arguments_that_can_be() {
    for (src, expected) in [
        (
            "enum Two<T> { Both(T, T) }\nenum Expr { Pair(Two<Expr>) }",
            vec![(Code::NeverConstructed, (2, 6))],
        ),
        (
            "enum Two<T> { Both(T, T) }\nenum Expr {\n    Lit(i64)\n    Pair(Two<Expr>)\n}",
            vec![],
        ),
        (
            "class K {\n    x: i64\n}\nenum Two<T> { Both(T, T) }\nenum F<T> { A(Two<Two<K>>, Two<T>, F<T>) }",
            vec![(Code::NeverConstructed, (5, 6))],
        ),
        // The chain reaches `Two<Leaf>` only if a class argument is held unbuildable.
        (
            "class Leaf {\n    e: Expr\n}\nenum Two<T> { Both(T, T) }\nenum Expr { Pair(Two<Leaf>) }",
            vec![(Code::NeverConstructed, (5, 6))],
        ),
        // `A` needs `C` through an argument, so only `B`, which needs nothing else, is reported.
        (
            "enum Two<T> { Both(T, T) }\nenum A { X(Two<C>) }\nclass C {\n    b: B\n}\nenum B { Y(B) }",
            vec![(Code::NeverConstructed, (6, 6))],
        ),
        (
            "enum Nest<T> { A(Nest<[T]>) }",
            vec![(Code::NeverConstructed, (1, 6))],
        ),
        // A class no value can be built of is reported when an argument hides its cycle,
        // and `E`, which only needs it, is not.
        (
            "class C<T> {\n    x: T\n}\nclass B {\n    c: C<B>\n}\nenum E { A(B) }",
            vec![(Code::NeverConstructed, (4, 7))],
        ),
        (
            "class Box<T> {\n    v: T\n}\nclass C {\n    b: Box<C>\n}",
            vec![(Code::NeverConstructed, (4, 7))],
        ),
        // `Ignore<Bad>` can be built, so `E` does not need a `Bad` and is reported on its own.
        (
            "enum Ignore<T> { A }\nenum Bad { A(Bad) }\nenum E { A(E, Ignore<Bad>) }",
            vec![
                (Code::NeverConstructed, (2, 6)),
                (Code::NeverConstructed, (3, 6)),
            ],
        ),
        // An argument a declaration ignores need not be buildable, and one that becomes
        // buildable later changes which application a field holds.
        (
            "enum Opt<T> {\n    No\n    Yes(T)\n}\nenum List {\n    Cons(i64, Opt<List>)\n}",
            vec![],
        ),
        (
            "class Box<T> {\n    v: T?\n}\nclass Node {\n    next: Box<Node>\n}",
            vec![],
        ),
        ("enum Ignore<T> { A }\nenum E { A(Ignore<E>) }", vec![]),
        (
            "class Box<T> {\n    v: T\n}\nenum E { A }\nenum Good { A(Box<E>) }",
            vec![],
        ),
        // `E` is built after `Good` is first tried, which then holds `Box<E>` rather than the
        // `Box` of an argument that cannot be built.
        (
            "class Box<T> {\n    v: T\n}\nenum E { A(i64) }\nenum Good { A(Box<E>) }",
            vec![],
        ),
        // Building `E` counts the field that consulted it twice as buildable once.
        (
            "enum E { A(i64) }\nenum Two<T, U> { A(T, U) }\nenum Bad { A(Two<E, E>, Bad) }",
            vec![(Code::NeverConstructed, (3, 6))],
        ),
        (
            "enum Pair<T> { P(T, T) }\nenum Tree {\n    Leaf\n    Node(Pair<Tree>)\n}",
            vec![],
        ),
        (
            "enum Ignore<T> { A }\nenum Bad { A(Bad) }\nenum Good { A(Ignore<Bad>) }",
            vec![(Code::NeverConstructed, (2, 6))],
        ),
        // A field whose type is unknown builds nothing, and is passed over on the chain.
        (
            "class C {\n    a: Zz\n    b: E\n}\nenum E { A(C) }",
            vec![
                (Code::UnknownType, (2, 8)),
                (Code::NeverConstructed, (5, 6)),
            ],
        ),
    ] {
        codes(&format!("{src}\nfn main() {{}}"), &expected);
    }
}

/// A key that `needs` or the chain asks for after `run` ends is built as `run` would build it,
/// so the classes that need it are reported as the others are.
#[test]
fn generic_keys_found_after_the_fixed_point_should_be_built_before_they_are_read() {
    for (src, expected) in [
        // `needs` adds the key `C<X, N>` after `run`, with `X` built late. That key is built.
        // So the chain skips `N.c`, reports `N` through `N.w`, and does not stop at `C`.
        (
            "class Y {\n}\nclass X {\n    y: Y\n}\nclass C<A, B> {\n    y: Y\n}\nclass W<T> {\n    t: T\n}\nclass N {\n    c: C<X, N>\n    w: W<N>\n}",
            vec![(Code::NeverConstructed, (12, 7))],
        ),
        // The flags of this key changed after its field was found buildable. The chain asks
        // for `C<X, N>` with `X` built, but `run` interned only `C<X, N>` with `X` not built.
        (
            "class Y {\n}\nclass X {\n    y: Y\n}\nclass C<A, B> {\n}\nclass W<T> {\n    c: C<X, T>\n    t: T\n}\nclass N {\n    w: W<N>\n}",
            vec![(Code::NeverConstructed, (12, 7))],
        ),
        // `U` needs nothing but itself once `C<X, Bad>` is known to be built. A late key that
        // stays unbuilt makes `U` seem to need `Bad`, and then `U` is not reported.
        (
            "class Y {\n}\nclass X {\n    y: Y\n}\nclass C<A, B> {\n    y: Y\n}\nclass W<T> {\n    t: T\n}\nclass Bad {\n    w: W<Bad>\n}\nclass U {\n    c: C<X, Bad>\n    w: W<U>\n}",
            vec![
                (Code::NeverConstructed, (12, 7)),
                (Code::NeverConstructed, (15, 7)),
            ],
        ),
        // The two programs of issue 178, in which only `K3` and only `K5` cannot be built.
        (
            "class P {\n}\nclass Q {\n}\nclass K0<T0> {\n    f1: K2<K1<P, Q>, K2<[T0], T0>>\n}\nclass K1<T0, T1> {\n}\nclass K2<T0, T1> {\n}\nclass K3<T0, T1> {\n    f1: K4<K3<T1, K0<T0>>, K1<Q, [T0]>>\n}\nclass K4<T0, T1> {\n    f0: K1<K1<K3<T1, P>, K0<Q>>, K3<T1, T1>>\n    f1: T0\n}",
            vec![(Code::NeverConstructed, (12, 7))],
        ),
        (
            "class P {\n}\nclass Q {\n}\nclass K0 {\n}\nclass K1<T0, T1, T2> {\n}\nclass K2<T0, T1, T2> {\n    f0: K1<K3<[T1], T2>, K1<K1<T2, T2, T0>, i64, T0>, [K3<T1, T0>]>\n}\nenum K3<T0, T1> {\n}\nclass K4<T0, T1> {\n    f0: T0\n}\nclass K5<T0> {\n    f0: K2<K4<K1<P, P, T0>, K0>, K5<i64>, K1<[Q], [i64], [Q]>>\n    f1: K4<K5<K0>, K1<K3<T0, T0>, [P], K0>>\n}",
            vec![
                (Code::EmptyEnum, (12, 6)),
                (Code::NeverConstructed, (17, 7)),
            ],
        ),
    ] {
        codes(&format!("{src}\nfn main() {{}}"), &expected);
    }
}

/// A field that cannot be built yet is scanned again only once a key it consulted is built, so
/// an enum of `n` fields, each of an enum built later or of a class wrapping one, costs a few
/// scans per field.
#[test]
fn the_can_be_built_fixed_point_should_scan_linearly() {
    let scans = |n: usize, wrap: &str| {
        let names: Vec<String> = (0..n).map(|i| format!("E{i}")).collect();
        let fields: Vec<String> = names.iter().map(|name| wrap.replace('_', name)).collect();
        let src = format!(
            "class Box<T> {{\n    v: T\n}}\nenum {} {{ A(i64) }}\nenum Root {{ A({}) }}\nfn main() {{}}",
            names.join(" { A(i64) }\nenum "),
            fields.join(", ")
        );
        crate::check::enums::SCANS.set(0);
        clean(&src);
        crate::check::enums::SCANS.get()
    };
    for wrap in ["_", "Box<_>"] {
        assert_near_linear(scans(4, wrap), scans(8, wrap), scans(12, wrap));
    }
}

/// A list after a name that takes none is reported at the list in every expression form, and
/// one after a value at the value.
#[test]
fn type_arguments_in_expressions_should_be_reported_at_the_list() {
    let arguments = |at| vec![(Code::TypeArguments, at)];
    for (src, expected) in [
        ("fn g() { print<i64>(1) }", arguments((1, 15))),
        (
            "fn g(xs: [i64]) -> i64 { xs.len<i64>() }",
            arguments((1, 32)),
        ),
        (
            "fn g(xs: [i64]) -> i64 { xs<i64>.len() }",
            arguments((1, 26)),
        ),
        ("fn g(p: Point) -> i64 { p<i64>.x }", arguments((1, 25))),
        (
            "fn h() -> i64 { 1 }\nfn g() -> i64 { h<i64>() }",
            arguments((2, 18)),
        ),
        (
            "const N = 1\nfn g(a: i64) -> bool { N<a>(1) }",
            arguments((2, 24)),
        ),
        (
            "class K {\n    fn m(self) -> i64 { 1 }\n}\nfn g(k: K) -> i64 { k.m<i64>() }",
            arguments((4, 24)),
        ),
        ("enum E { A }\nfn g() -> E { E<i64>.A }", arguments((2, 16))),
        ("fn g(x: i64) -> f64 { f64<i64>(x) }", arguments((1, 26))),
        (
            "fn g() -> f64 { math.sqrt<f64>(2.0) }",
            vec![(Code::MathIdiom, (1, 17)), (Code::TypeArguments, (1, 26))],
        ),
        (
            "fn g(b: bool) { b.m<i64>() }",
            vec![(Code::NoMember, (1, 19))],
        ),
        (
            "fn g(p: Point) { p.m<i64>() }",
            vec![(Code::NoMember, (1, 20))],
        ),
        (
            "fn g() { zz<i64>(1) }",
            vec![(Code::FunctionNotFound, (1, 10))],
        ),
        (
            "fn g() { zz.m<i64>() }",
            vec![(Code::BindingNotFound, (1, 10))],
        ),
        ("fn g() -> i64 { \"s\".len<i64>() }", arguments((1, 24))),
        (
            "fn g(xs: [i64]) -> i64 { xs<i64>.len<i64>() }",
            arguments((1, 26)),
        ),
        (
            "fn g() -> i64 { Point<i64>.x }",
            vec![(Code::TypeAsValue, (1, 17)), (Code::TypeArguments, (1, 22))],
        ),
        (
            "enum E { A }\nfn g() { E<i64>() }",
            vec![
                (Code::FunctionNotFound, (2, 10)),
                (Code::TypeArguments, (2, 11)),
            ],
        ),
        (
            "enum E { A(i64) }\nfn g() -> E { E.A<i64>(1) }",
            arguments((2, 18)),
        ),
        (
            "enum E {\n    A\n    fn m(self) -> i64 { 1 }\n}\nfn g() -> i64 { E<i64>.m<i64>() }",
            vec![
                (Code::TypeArguments, (5, 18)),
                (Code::MemberKind, (5, 24)),
                (Code::TypeArguments, (5, 25)),
            ],
        ),
        (
            "enum E {\n    A\n    fn m(self) -> i64 { 1 }\n}\nfn g(e: E) -> i64 { e.m<i64>() }",
            arguments((5, 24)),
        ),
        (
            "enum E {\n    A\n    fn m(self) -> i64 { 1 }\n}\nfn g() -> i64 { E.m<i64>() }",
            vec![(Code::MemberKind, (5, 19)), (Code::TypeArguments, (5, 20))],
        ),
    ] {
        codes(&with_decls(src), &expected);
    }
}

/// A member the receiver does not have is reported alone, and so is a path's receiver that is
/// no idiom: no list is reported after either.
#[test]
fn an_unknown_member_should_be_reported_alone() {
    for (src, expected) in [
        (
            "fn g() { [1].what<i64>() }",
            vec![(Code::NoMember, (1, 14))],
        ),
        (
            "fn g() { \"s\".what<i64>() }",
            vec![(Code::NoMember, (1, 14))],
        ),
        ("fn g() { 1.what<i64>() }", vec![(Code::NoMember, (1, 12))]),
        (
            "enum E { A }\nfn g() { E.nope<i64>() }",
            vec![(Code::UnknownVariant, (2, 12))],
        ),
        (
            "fn g() { math.nope<f64>(1.0) }",
            vec![(Code::BindingNotFound, (1, 10))],
        ),
        (
            "fn g() { f64.nope<f64>(1.0) }",
            vec![(Code::TypeAsValue, (1, 10))],
        ),
        (
            "fn g() { i64.abs<i64>(1) }",
            vec![(Code::TypeAsValue, (1, 10))],
        ),
    ] {
        codes(&with_decls(src), &expected);
    }
}

/// The generic declarations the inference rows call, after `DECLS`.
const GENERIC: &str = "fn id<T>(x: T) -> T { x }\nfn first<T>(xs: [T]) -> T? {\n    if xs.len() == 0 { return None }\n    xs[0]\n}\nfn pair<T>(a: T, b: T) -> [T] { [a, b] }\nfn opt<T>(x: T?) -> T? { x }\nfn wrap<T>(x: T) -> [T] { [x] }\nfn make<T>() -> [T] { [] }\nfn make_ints() -> [i64] { [1] }\nfn fill<T>(xs: [T], s: Stack<T>) {}\nfn take<U>(x: U, n: i64) {}\nclass Pile<T> {\n    mut top: Cell<T>?\n    fn push(self, x: T) { self.top = Cell(value: x, next: self.top) }\n}\nclass Box<T> {\n    v: T\n    fn map_all<U>(self, u: U) -> [U] { [u] }\n    fn make<U>(self) -> [U] { [] }\n}\nenum Tree<T> {\n    Leaf\n    Node(Tree<T>, T, Tree<T>)\n}\nenum Opt2<T> {\n    S(T)\n    N\n}\nenum Wrap<T> { W(T) }\nenum E<T> {\n    A(T)\n    fn map<U>(self, u: U) -> [U] { [u] }\n}";

/// `line` on line 1, then `GENERIC` and `DECLS`.
pub(super) fn with_generics(line: &str) -> String {
    format!("{line}\n{GENERIC}\n{DECLS}")
}

/// The type arguments of the first call of `callee` in `src`'s `tir`, before `mono`, then the
/// call's type, each as a program writes it. `callee` names a function (`Box.map_all` for a
/// method), or a class or an enum, whose construction's arguments are its type's.
fn call_types(src: &str, callee: &str) -> Vec<String> {
    let (program, ds) = checked(src);
    assert!(ds.is_empty(), "{ds:?}");
    let program = program.expect("a program without diagnostics should have a tir");
    let mut exprs = Vec::new();
    for function in &program.functions {
        function.body.exprs(&mut exprs);
    }
    let call = exprs.into_iter().find_map(|e| {
        let (name, types) = match (&e.kind, e.ty.kind()) {
            (tir::ExprKind::Call { func, types, .. }, _) => {
                (&program.functions[func.0].name, *types)
            }
            (tir::ExprKind::New { .. }, TypeKind::Class(id, args)) => {
                (&program.classes[id.0].name, args)
            }
            (tir::ExprKind::Variant { .. }, TypeKind::Enum(id, args)) => {
                (&program.enums[id.0].name, args)
            }
            _ => return None,
        };
        (name == callee).then_some((types, e.ty))
    });
    let (types, ty) = call.unwrap_or_else(|| panic!("`{callee}` should be called in {src}"));
    let mut names: Vec<_> = types.get().iter().map(|&t| program.type_name(t)).collect();
    names.push(program.type_name(ty));
    names
}

/// Each row's `callee` is called with the type arguments and type after it (see `call_types`).
pub(super) fn infers(rows: &[(&str, &str, &[&str])]) {
    for &(line, callee, expected) in rows {
        assert_eq!(call_types(&with_generics(line), callee), expected, "{line}");
    }
}

/// Arguments and a method's receiver bind the callee's type parameters, or written type
/// arguments give them; the result's type is the return type with them substituted.
#[test]
fn arguments_should_bind_type_parameters() {
    infers(&[
        (
            "fn g() -> i64? { first([3, 9, 2]) }",
            "first",
            &["i64", "i64?"],
        ),
        (
            "fn g() {\n    let x = first([3, 9, 2])\n}",
            "first",
            &["i64", "i64?"],
        ),
        (
            "fn g() -> string? { first<string>([]) }",
            "first",
            &["string", "string?"],
        ),
        (
            "fn g(x: [string]) -> string? { first(x) }",
            "first",
            &["string", "string?"],
        ),
        (
            "fn g() -> Cell<i64> { Cell(value: 1, next: None) }",
            "Cell",
            &["i64", "Cell<i64>"],
        ),
        (
            "fn g() -> Stack<i64> { Stack<i64>(top: None) }",
            "Stack",
            &["i64", "Stack<i64>"],
        ),
        // `Pile.push` builds a `Cell<T>` of its own parameter.
        ("fn g() {}", "Cell", &["T", "Cell<T>"]),
        (
            "fn g(s: Pile<string>) { s.push(\"a\") }",
            "Pile.push",
            &["string", "unit"],
        ),
        (
            "fn g() -> [string] { Box(v: 1).map_all(\"a\") }",
            "Box.map_all",
            &["i64", "string", "[string]"],
        ),
        (
            "fn g() -> [f64] { Box(v: 1).map_all<f64>(2.5) }",
            "Box.map_all",
            &["i64", "f64", "[f64]"],
        ),
        (
            "fn g() -> [string] { E.A(1).map(\"a\") }",
            "E.map",
            &["i64", "string", "[string]"],
        ),
        (
            "fn g() -> Tree<i64> { Tree.Node(Tree.Leaf, 5, Tree.Leaf) }",
            "Tree",
            &["i64", "Tree<i64>"],
        ),
        (
            "fn g() -> Tree<i64> { Tree<i64>.Leaf }",
            "Tree",
            &["i64", "Tree<i64>"],
        ),
        (
            "enum Ignore<T> { A }\nenum F { A(Ignore<F>) }\nfn g() -> F { F.A(Ignore.A) }",
            "Ignore",
            &["F", "Ignore<F>"],
        ),
        // Written type arguments follow the receiver's.
        (
            "class Duo<T> {\n    v: T\n    fn both<A, B>(self, a: A, b: B) -> B { b }\n}\nfn g() -> string { Duo(v: true).both<i64, string>(1, \"s\") }",
            "Duo.both",
            &["bool", "i64", "string", "string"],
        ),
        // A type argument that names no parameter must be the argument's.
        (
            "class Two<A, B> {\n    a: A\n    b: B\n}\nfn snd<T>(t: Two<T, i64>) -> T { t.a }\nfn g(t: Two<string, i64>) -> string { snd(t) }",
            "snd",
            &["string", "string"],
        ),
        (
            "class Two<A, B> {\n    a: A\n    b: B\n}\nfn g() {\n    let t = Two(a: 1, b: \"s\")\n}",
            "Two",
            &["i64", "string", "Two<i64, string>"],
        ),
        (
            "enum Lab<T> { P(x: T, y: T) }\nfn g() {\n    let p = Lab.P(x: 1, y: 2)\n}",
            "Lab",
            &["i64", "Lab<i64>"],
        ),
    ]);
}

/// The expected type binds what no other argument does.
#[test]
fn the_expected_type_should_bind_the_rest() {
    infers(&[
        (
            "fn g() {\n    let s: Stack<string> = Stack(top: None)\n}",
            "Stack",
            &["string", "Stack<string>"],
        ),
        (
            "fn g() {\n    let t: Tree<string> = Tree.Leaf\n}",
            "Tree",
            &["string", "Tree<string>"],
        ),
        ("fn g() {\n    let x: f64 = id(1)\n}", "id", &["f64", "f64"]),
        (
            "fn g() {\n    let xs: [i64] = id(make())\n}",
            "make",
            &["i64", "[i64]"],
        ),
        // What of the return type meets the expected type binds, each part on its own.
        (
            "class Two<A, B> {\n    a: A\n    b: B\n}\nfn two<A, B>(x: A, y: B) -> Two<A, B> { Two(a: x, b: y) }\nfn g() {\n    let x: Two<f64, [i64]> = two(1, [])\n}",
            "two",
            &["f64", "[i64]", "Two<f64, [i64]>"],
        ),
    ]);
}

/// A call of a method with type parameters of its own, on a local or a field of one, waits for
/// context as a call of a generic function does.
#[test]
fn generic_methods_should_wait_for_context() {
    infers(&[
        (
            "fn g(b: Box<i64>) {\n    let xs: [i64] = id(b.make())\n}",
            "Box.make",
            &["i64", "i64", "[i64]"],
        ),
        (
            "fn g(b: Box<i64>) {\n    let xs: [i64] = b.make()\n}",
            "Box.make",
            &["i64", "i64", "[i64]"],
        ),
        (
            "class Holder {\n    b: Box<i64>\n}\nfn g(h: Holder) {\n    let xs: [i64] = id(h.b.make())\n}",
            "Box.make",
            &["i64", "i64", "[i64]"],
        ),
        (
            "fn g(e: E<i64>) {\n    let xs: [[string]] = id(e.map([]))\n}",
            "E.map",
            &["i64", "[string]", "[[string]]"],
        ),
        // `?.` calls the method on the payload, which waits as a plain receiver does.
        (
            "fn g(b: Box<i64>?) {\n    let xs: [i64]? = opt(b?.make())\n}",
            "Box.make",
            &["i64", "i64", "[i64]"],
        ),
        // Any receiver is checked ahead of the call, once: a temporary, a call's result, `self`.
        (
            "fn g() {\n    let xs: [i64] = id(Box(v: 1).make())\n}",
            "Box.make",
            &["i64", "i64", "[i64]"],
        ),
        (
            "fn mk() -> Box<i64> { Box(v: 1) }\nfn g() {\n    let xs: [i64] = id(mk().make())\n}",
            "Box.make",
            &["i64", "i64", "[i64]"],
        ),
        (
            "class Cup<T> {\n    v: T\n    fn make<U>(self) -> [U] { [] }\n    fn all(self) -> [i64] { id(self.make()) }\n}",
            "Cup.make",
            &["T", "i64", "[i64]"],
        ),
        (
            "fn g(b: Box<i64>, c: bool) {\n    let xs: [i64] = id(if c { b.make() } else { [] })\n}",
            "Box.make",
            &["i64", "i64", "[i64]"],
        ),
        // Each receiver checked ahead is kept for its own method.
        (
            "fn g(b: Box<i64>, e: E<i64>) {\n    let xs: [[[i64]]] = pair(b.make(), e.map(make()))\n}",
            "E.map",
            &["i64", "[i64]", "[[i64]]"],
        ),
        // A block's value is ranked in the block's scope, which its `let`s are in.
        (
            "fn g(b: Box<i64>) {\n    let xs: [i64] = id({ let n = 1; b.make() })\n}",
            "Box.make",
            &["i64", "i64", "[i64]"],
        ),
        (
            "fn g(c: i64) {\n    let xs: [i64] = id({ let c = Box(v: 1); c.make() })\n}",
            "Box.make",
            &["i64", "i64", "[i64]"],
        ),
        // Each call that ranks the block ranks its value in that scope.
        (
            "fn g() {\n    let xs: [i64] = id(id({ let c = Box(v: 1); c.make() }))\n}",
            "Box.make",
            &["i64", "i64", "[i64]"],
        ),
        // A method without type parameters of its own takes its receiver's, so does not wait.
        (
            "class Jar<T> {\n    v: T\n    fn put(self, xs: [T]) -> [T] { xs }\n}\nfn g(j: Jar<i64>) {\n    let xs = id(j.put([]))\n}",
            "Jar.put",
            &["i64", "[i64]"],
        ),
        // A variant waits whether its payload is labeled or not.
        (
            "enum Tag<T> { P(x: [T]) }\nfn g() {\n    let p: Tag<i64> = id(Tag.P(x: []))\n}",
            "Tag",
            &["i64", "Tag<i64>"],
        ),
        (
            "fn g() {\n    let w: Wrap<[i64]> = id(Wrap.W([]))\n}",
            "Wrap",
            &["[i64]", "Wrap<[i64]>"],
        ),
    ]);
    // `?.` on a value that is not optional is reported once, and the method still waits.
    let src = "fn g(b: Box<i64>) {\n    let xs: [i64]? = opt(b?.make())\n}";
    codes(
        &with_generics(src),
        &[(Code::SafeAccessNotOptional, (2, 26))],
    );
}

/// An `if let` and a `match` wait as an `if` does, each branch ranked in the scope that holds
/// what its pattern binds.
#[test]
fn if_let_and_match_should_wait_for_context() {
    infers(&[
        (
            "fn g(b: Box<i64>, o: i64?) {\n    let xs: [i64] = id(if let n = o { b.make() } else { [] })\n}",
            "Box.make",
            &["i64", "i64", "[i64]"],
        ),
        (
            "fn g(o: Box<i64>?) {\n    let xs: [i64] = id(if let n = o { n.make() } else { [] })\n}",
            "Box.make",
            &["i64", "i64", "[i64]"],
        ),
        (
            "fn g(o: Box<i64>?) {\n    let xs: [i64] = id(id(if let n = o { n.make() } else { [] }))\n}",
            "Box.make",
            &["i64", "i64", "[i64]"],
        ),
        (
            "enum Slot {\n    Has(Box<i64>)\n    Empty\n}\nfn g(b: Box<i64>, e: Slot) {\n    let xs: [i64] = id(match e {\n        Has(x) => []\n        Empty => b.make()\n    })\n}",
            "Box.make",
            &["i64", "i64", "[i64]"],
        ),
        (
            "enum Slot {\n    Has(Box<i64>)\n    Empty\n}\nfn g(e: Slot) {\n    let xs: [i64] = id(match e {\n        Has(x) => x.make()\n        Empty => []\n    })\n}",
            "Box.make",
            &["i64", "i64", "[i64]"],
        ),
        (
            "enum Slot {\n    Has(Box<i64>)\n    Empty\n}\nfn g(e: Slot) {\n    let xs: [i64] = id(match e {\n        Has(x) => {\n            let y = 1\n            x.make()\n        }\n        Empty => []\n    })\n}",
            "Box.make",
            &["i64", "i64", "[i64]"],
        ),
        (
            "enum Slot {\n    Has(Box<i64>)\n    Empty\n}\nfn g(e: Slot) {\n    let xs: [i64] = id(match e {\n        Has(x) if x.v > 0 => x.make()\n        _ => []\n    })\n}",
            "Box.make",
            &["i64", "i64", "[i64]"],
        ),
    ]);
    // A guard sees the pattern's `x`, not the `x` its body declares after it.
    let arm = "Has(x) if x.v > 0 => {\n            let x = 1\n            [x]\n        }";
    let missing = "Has(x) if y > 0 => {\n            let y = 1\n            [y]\n        }";
    for (open, close) in [("id(", ")"), ("id(id(", "))")] {
        let src = |arm: &str| {
            format!(
                "enum Slot {{\n    Has(Box<i64>)\n    Empty\n}}\nfn g(e: Slot) {{\n    let xs: [i64] = {open}match e {{\n        {arm}\n        _ => []\n    }}{close}\n}}"
            )
        };
        clean(&with_generics(&src(arm)));
        codes(
            &with_generics(&src(missing)),
            &[(Code::BindingNotFound, (7, 19))],
        );
    }
}

/// Ranking an `if let` or a `match` checks its value and binds each ranked branch's pattern
/// once: what the pattern reports, it reports once, and each name it binds is one local.
#[test]
fn ranked_patterns_should_report_and_bind_once() {
    const SLOT: &str =
        "enum Slot {\n    Has(Box<i64>)\n    Empty\n}\nenum One { Only(Box<i64>) }\n";
    for (body, expected) in [
        (
            "fn g(b: Box<i64>, e: Slot) {\n    let xs: [i64] = id(if let Slot.Nope(x) = e { b.make() } else { [] })\n}",
            vec![(Code::UnknownVariant, (7, 36))],
        ),
        (
            "fn g(b: Box<i64>, e: Slot) {\n    let xs: [i64] = id(match e {\n        Nope(x) => b.make()\n        _ => []\n    })\n}",
            vec![(Code::UnknownVariant, (8, 9))],
        ),
        // A block arm's body starts at `(`, but ranking and the check keep it by its block.
        (
            "fn g(b: Box<i64>, e: Slot) {\n    let xs: [i64] = id(match e {\n        Nope(x) => ({ b.make() })\n        _ => []\n    })\n}",
            vec![(Code::UnknownVariant, (8, 9))],
        ),
        (
            "fn g(e: One) {\n    let xs: [i64] = id(if let Only(x) = e { x.make() } else { [] })\n}",
            vec![(Code::AlwaysMatches, (7, 31))],
        ),
        // The value and the scrutinee are checked once, by ranking.
        (
            "fn g(b: Box<i64>) {\n    let xs: [i64] = id(if let n = nope { b.make() } else { [] })\n}",
            vec![(Code::BindingNotFound, (7, 35))],
        ),
        (
            "fn g(b: Box<i64>) {\n    let xs: [i64] = id(match nope {\n        Has(x) => b.make()\n        _ => []\n    })\n}",
            vec![(Code::BindingNotFound, (7, 30))],
        ),
    ] {
        codes(&with_generics(&format!("{SLOT}{body}")), &expected);
    }
    for (body, name) in [
        (
            "fn g(o: Box<i64>?) {\n    let xs: [i64] = id(if let n = o { n.make() } else { [] })\n}",
            "n",
        ),
        (
            "fn g(o: Box<i64>?) {\n    let xs: [i64] = id(id(if let n = o { n.make() } else { [] }))\n}",
            "n",
        ),
        (
            "fn g(e: Slot) {\n    let xs: [i64] = id(id(match e {\n        Has(x) if x.v > 0 => x.make()\n        _ => []\n    }))\n}",
            "x",
        ),
        (
            "fn g(e: Slot) {\n    let xs: [i64] = id(match e {\n        Has(x) => ({ x.make() })\n        _ => []\n    })\n}",
            "x",
        ),
    ] {
        let (program, ds) = checked(&with_generics(&format!("{SLOT}{body}")));
        assert!(ds.is_empty(), "{body}: {ds:?}");
        let program = program.expect("a program without diagnostics should have a tir");
        let g = program.functions.iter().find(|f| f.name == "g");
        let g = g.expect("the program should hold `g`");
        let bound = g.locals.iter().filter(|l| l.name.as_deref() == Some(name));
        assert_eq!(bound.count(), 1, "{body}");
    }
}

/// A `match` or an `if` that waits for context is still checked when the call cannot infer its
/// type parameter. It reports its own mistakes beside the call's, and its branches do not report
/// what the call reports.
#[test]
fn a_waiting_branch_of_a_failed_call_should_report_its_own_mistakes() {
    for (body, expected) in [
        (
            "fn g() {\n    let xs = id(match 1.0 { _ => [] })\n}",
            vec![(Code::CannotInfer, (2, 14)), (Code::CannotMatch, (2, 23))],
        ),
        (
            "fn g() {\n    let ys = id(match true { true => [] })\n}",
            vec![(Code::CannotInfer, (2, 14)), (Code::NonExhaustive, (2, 17))],
        ),
        (
            "fn g() {\n    let xs = id(if 1.0 { [] } else { [] })\n}",
            vec![(Code::CannotInfer, (2, 14)), (Code::Mismatch, (2, 20))],
        ),
        // What fails inside the branch for the call's failure is not reported again.
        (
            "fn g() {\n    let xs = id(match true { _ => [id([])] })\n}",
            vec![(Code::CannotInfer, (2, 14))],
        ),
        (
            "fn g() {\n    let xs = id([[[]]])\n}",
            vec![(Code::CannotInfer, (2, 14))],
        ),
        (
            "fn g() {\n    let xs = id([id([])])\n}",
            vec![(Code::CannotInfer, (2, 14))],
        ),
    ] {
        codes(&with_generics(body), &expected);
    }
}

/// Each binding is checked once, and a failed one undoes only what it bound, so inference's
/// work grows with a call's type parameters, not with their square.
#[test]
fn inference_should_check_each_binding_once() {
    let visits = |n: usize| {
        let types: Vec<_> = (0..n).map(|i| format!("T{i}")).collect();
        let params: Vec<_> = (0..n).map(|i| format!("x{i}: T{i}")).collect();
        let src = format!(
            "fn f<{}>({}) {{}}\nfn main() {{ f({}) }}",
            types.join(", "),
            params.join(", "),
            vec!["true"; n].join(", ")
        );
        generic::VISITS.set(0);
        clean(&src);
        generic::VISITS.get()
    };
    assert_eq!(visits(64), 64);
    assert_eq!(visits(128), 128);
}

/// An element pattern 20 arrays deep, so a walk of it for each element would show. Once the
/// first element binds `T`, a later one adds no walk. A `P<i64, bool>` before it binds `T`,
/// then fails at `bool` and undoes the binding: it adds no walk either.
#[test]
fn array_elements_should_meet_a_bound_element_type_with_no_new_walk() {
    let walked = |elements: &str| {
        let (open, close) = ("[".repeat(20), "]".repeat(20));
        let src = format!(
            "class P<A, B> {{\n    a: A\n    b: B\n}}\n\
             fn take<T>(xs: [{open}P<T, T>?{close}]) -> i64 {{ xs.len() }}\n\
             fn main() {{\n    let b: {open}P<i64, bool>?{close} = {open}P(a: 1, b: true){close}\n    \
             let x: {open}P<i64, i64>?{close} = {open}P(a: 1, b: 2){close}\n    \
             print(take([{elements}]))\n}}"
        );
        generic::WALKED.set(0);
        let mismatches = diags(&src).len();
        (generic::WALKED.get(), mismatches)
    };
    let (one, _) = walked("x");
    assert_eq!(walked(&vec!["x"; 100].join(", ")), (one, 0));
    let (bad, _) = walked("b, x");
    assert_eq!(walked(&format!("{}x", "b, ".repeat(100))), (bad, 100));
}

/// A call that leaves type parameters unbound looks at each declared name once to find the
/// reported ones, so its work grows with them, not with their square; a call that binds them
/// all looks at none.
#[test]
fn reported_type_parameters_should_be_found_in_one_pass() {
    let scanned = |n: usize, args: &str| {
        let types: Vec<_> = (0..n).map(|i| format!("T{i}")).collect();
        let params: Vec<_> = (0..n).map(|i| format!("x{i}: T{i}")).collect();
        let src = format!(
            "fn f<{}>({}) {{}}\nfn main() {{ f({args}) }}",
            types.join(", "),
            params.join(", "),
        );
        generic::SCANNED.set(0);
        checked(&src);
        generic::SCANNED.get()
    };
    let none = |n: usize| vec!["[]"; n].join(", ");
    assert_eq!(scanned(500, &none(500)), 500);
    assert_eq!(scanned(1000, &none(1000)), 1000);
    assert_eq!(scanned(500, &vec!["true"; 500].join(", ")), 0);
}

/// A type parameter is found in one step, however many a body declares: indexing them is one
/// step each, and each lookup, by a type or by a type argument that may be a value, is one more.
#[test]
fn type_parameters_should_be_found_in_one_step() {
    let steps = |n: usize| {
        let types: Vec<_> = (0..n).map(|i| format!("T{i}")).collect();
        let src = format!(
            "fn f<{}>(a: i64, x: T{}) {{\n{}}}\nfn main() {{}}",
            types.join(", "),
            n - 1,
            "a<Missing>(1)\n".repeat(n),
        );
        PARAM_STEPS.set(0);
        checked(&src);
        PARAM_STEPS.get()
    };
    assert_eq!(steps(500), 1502);
    assert_eq!(steps(1000), 3002);
}

/// Work kept ahead of a call is found by its span in one step, so finding it grows with the
/// call's arguments, not with their square: one step per receiver and per block.
#[test]
fn work_kept_ahead_should_be_found_in_one_step() {
    let inspected = |n: usize, arg: &str, ty: &str| {
        let params: Vec<_> = (0..n).map(|i| format!("x{i}: T")).collect();
        let src = with_generics(&format!(
            "fn many<T>({}) -> T {{ x0 }}\nfn g(b: Box<i64>) {{\n    let x: {ty} = many({})\n}}",
            params.join(", "),
            vec![arg; n].join(", ")
        ));
        generic::INSPECTED.set(0);
        clean(&src);
        generic::INSPECTED.get()
    };
    for (arg, ty) in [("b.make()", "[i64]"), ("{ let n = true; n }", "bool")] {
        assert_eq!(inspected(64, arg, ty), 64, "{arg}");
        assert_eq!(inspected(128, arg, ty), 128, "{arg}");
    }
}

/// A literal-like argument takes its type from the others, as an array literal's element
/// does from its neighbours.
#[test]
fn literal_like_arguments_should_follow_the_others() {
    infers(&[
        ("fn g() -> [i64] { pair(1, 2) }", "pair", &["i64", "[i64]"]),
        (
            "fn g() -> [f64] { pair(1, 2.5) }",
            "pair",
            &["f64", "[f64]"],
        ),
        (
            "fn g() -> [f64] { pair(2.5, 1) }",
            "pair",
            &["f64", "[f64]"],
        ),
        (
            "fn g() -> [f64] { pair(-1, 2.5) }",
            "pair",
            &["f64", "[f64]"],
        ),
        (
            "fn g(x: f64) -> [f64] { pair(x, 1) }",
            "pair",
            &["f64", "[f64]"],
        ),
        (
            "fn g() -> [[i64]] { pair(make_ints(), []) }",
            "pair",
            &["[i64]", "[[i64]]"],
        ),
        (
            "fn g() {\n    let xs: [f64] = id([1, 2])\n}",
            "id",
            &["[f64]", "[f64]"],
        ),
        (
            "fn g() {\n    let xs: [f64?] = id([1, None])\n}",
            "id",
            &["[f64?]", "[f64?]"],
        ),
        (
            "fn g(s: Stack<f64>) { fill([1, 2], s) }",
            "fill",
            &["f64", "unit"],
        ),
        (
            "fn g(x: f64) {\n    let xs = pair([1], [1, x])\n}",
            "pair",
            &["[f64]", "[[f64]]"],
        ),
        (
            "fn g(x: f64) {\n    let xs = pair(1, -x)\n}",
            "pair",
            &["f64", "[f64]"],
        ),
        (
            "fn g() {\n    let xs = pair(1 + 2, 2.5)\n}",
            "pair",
            &["f64", "[f64]"],
        ),
    ]);
}

/// A `T?` parameter takes an optional's payload, or a value the call wraps.
#[test]
fn an_optional_parameter_should_take_the_payload() {
    infers(&[
        ("fn g() -> i64? { opt(5) }", "opt", &["i64", "i64?"]),
        ("fn g(n: i64?) -> i64? { opt(n) }", "opt", &["i64", "i64?"]),
        ("fn g() {\n    let x = opt(5)\n}", "opt", &["i64", "i64?"]),
    ]);
}

/// An argument that needs context is checked against its parameter once the others bind it.
#[test]
fn arguments_that_need_context_should_wait_for_it() {
    infers(&[
        (
            "fn g() {\n    let xs: [i64] = id(id([]))\n}",
            "id",
            &["[i64]", "[i64]"],
        ),
        (
            "fn g(c: bool) {\n    let xs: [i64] = id(if c { [] } else { [] })\n}",
            "id",
            &["[i64]", "[i64]"],
        ),
        (
            "fn g(c: bool) -> [i64] {\n    let xs: [i64] = id(if c { return [] } else { [] })\n    xs\n}",
            "id",
            &["[i64]", "[i64]"],
        ),
        (
            "fn g() {\n    let xs: [i64] = id({ [] })\n}",
            "id",
            &["[i64]", "[i64]"],
        ),
        (
            "fn g() {\n    let xs: [[i64]] = id([[], []])\n}",
            "id",
            &["[[i64]]", "[[i64]]"],
        ),
        (
            "fn g() {\n    let s: Stack<i64> = id(Stack(top: None))\n}",
            "id",
            &["Stack<i64>", "Stack<i64>"],
        ),
        (
            "fn g() {\n    let w: Wrap<[i64]> = id(Wrap.W([]))\n}",
            "Wrap",
            &["[i64]", "Wrap<[i64]>"],
        ),
    ]);
}

/// An argument with no `[]`, `None` or generic name to bind binds on its own, whatever its
/// form.
#[test]
fn other_arguments_should_bind_on_their_own() {
    infers(&[
        (
            "fn g() {\n    let x = id(make_ints())\n}",
            "id",
            &["[i64]", "[i64]"],
        ),
        // A call given its type arguments.
        (
            "fn g() {\n    let x = id(make<i64>())\n}",
            "id",
            &["[i64]", "[i64]"],
        ),
        (
            "fn g(xs: [i64]) {\n    let n = id(xs.len())\n}",
            "id",
            &["i64", "i64"],
        ),
        (
            "fn g(c: bool) {\n    let n = id(if c { 1 } else { 2 })\n}",
            "id",
            &["i64", "i64"],
        ),
        (
            "fn g(b: Box<i64>) {\n    let x = id(b.v)\n}",
            "id",
            &["i64", "i64"],
        ),
    ]);
}

/// Whether an argument needs context goes by what its names resolve to: a call's callee by
/// its declaration, whatever local takes the name, and a local before a variant.
#[test]
fn context_should_follow_name_resolution() {
    infers(&[
        (
            "fn g() {\n    let make = 42\n    print(make)\n    let xs: [i64] = id(make())\n}",
            "id",
            &["[i64]", "[i64]"],
        ),
        (
            "fn g() -> i64 {\n    let Leaf = 1\n    id(Leaf)\n}",
            "id",
            &["i64", "i64"],
        ),
    ]);
}

/// Inside a generic body, the caller's parameters are fixed types that bind the callee's.
#[test]
fn callee_params_should_not_be_the_callers() {
    infers(&[
        (
            "fn g<T>(xs: [T]) -> T? { first(xs) }",
            "first",
            &["T", "T?"],
        ),
        ("fn h<T>(x: T) -> [T] { wrap(x) }", "wrap", &["T", "[T]"]),
    ]);
}

/// A field, a method's signature and a pattern's payload take the receiver's or the
/// scrutinee's arguments, and coverage splits the substituted payload.
#[test]
fn members_and_patterns_should_substitute_the_arguments() {
    for line in [
        "fn g(s: Stack<string>) -> Cell<string>? { s.top }",
        "fn g(s: Stack<i64>) { s.top = None }",
        "fn g(o: Opt2<bool>) -> i64 {\n    match o {\n        Opt2.S(true) => 1\n        Opt2.S(false) => 2\n        Opt2.N => 3\n    }\n}",
        "fn g(o: Opt2<i64>) -> i64 {\n    match o {\n        Opt2.S(n) => n\n        Opt2.N => 0\n    }\n}",
        "class Q<T> {\n    v: T\n    fn put(self, x: T) {}\n}\nfn g(q: Q<i64>) { q.put(1) }",
    ] {
        clean(&with_generics(line));
    }
}

/// An argument that may exit after one whose type names a parameter, or a generic enum's
/// variant, spills it, as for a call of a declaration that is not generic.
#[test]
fn generic_calls_should_spill_their_arguments() {
    contains(
        &with_generics("fn h<T>(x: T, c: bool) {\n    take(x, if c { return } else { 0 })\n}"),
        &["(let #2 x#0)"],
    );
    contains(
        &with_generics(
            "fn k(w: Wrap<string>, n: i64) {}\nfn h(s: string, c: bool) {\n    k(Wrap.W(s), if c { return } else { 0 })\n}",
        ),
        &["(let #2 (variant Wrap.W s#0))"],
    );
}

/// A generic call that cannot bind a parameter, or whose argument or list cannot fit, reports
/// that once, and so does each mistake a call of a declaration that is not generic can make.
#[test]
fn a_generic_call_that_does_not_check_should_report_once() {
    let infer = |at| vec![(Code::CannotInfer, at)];
    let arguments = |at| vec![(Code::TypeArguments, at)];
    let argument = |at| vec![(Code::TypeArgument, at)];
    for (line, expected) in [
        // The call's result meets the expected type after `n` binds `T`.
        (
            "fn g() {\n    let n: i64 = 1\n    let x: f64 = id(n)\n}",
            vec![(Code::Mismatch, (3, 18))],
        ),
        ("fn g() {\n    let x = first([])\n}", infer((2, 13))),
        ("fn g() {\n    let t = Tree.Leaf\n}", infer((2, 13))),
        // A later use binds nothing.
        (
            "fn g() {\n    let s = Pile(top: None)\n    s.push(1)\n}",
            infer((2, 13)),
        ),
        ("fn g() {\n    let x = id(make())\n}", infer((2, 13))),
        (
            "fn g(b: Box<i64>) {\n    let x = b.map_all([])\n}",
            infer((2, 15)),
        ),
        // An argument that cannot fit its parameter binds nothing, and is the call's error.
        (
            "fn g() {\n    let x = first(5)\n}",
            vec![(Code::Mismatch, (2, 19))],
        ),
        (
            "fn g() {\n    let x = pair(1, \"a\")\n}",
            vec![(Code::Mismatch, (2, 18))],
        ),
        (
            "fn g() {\n    let x = id(Leaf)\n}",
            vec![(Code::BareVariant, (2, 16))],
        ),
        (
            "fn f<T>(o: Opt2<T>) -> i64 {\n    match o { Opt2.N => 0 }\n}",
            vec![(Code::NonExhaustive, (2, 5))],
        ),
        (
            "fn g() -> Tree<i64> { Tree.Node<i64>(Tree.Leaf, 5, Tree.Leaf) }",
            arguments((1, 32)),
        ),
        (
            "fn g() {\n    let x = first<i64, string>([])\n}",
            arguments((2, 18)),
        ),
        (
            "fn g() {\n    let x = Box(v: 1).map_all<f64, f64>(2.5)\n}",
            arguments((2, 30)),
        ),
        // `never` binds only what nothing else does.
        (
            "fn g() {\n    let x = id(panic(\"x\"))\n}",
            argument((2, 16)),
        ),
        (
            "fn g() {\n    let x = pair(panic(\"x\"), 1)\n}",
            vec![(Code::NoValue, (2, 18))],
        ),
        // A variant's base is a type, which no assignment takes.
        (
            "fn g() {\n    Tree<i64>.Leaf = 1\n}",
            vec![(Code::TypeAsValue, (2, 5))],
        ),
        (
            "fn g() {\n    let x = id(1, 2)\n}",
            vec![(Code::ArgumentCount, (2, 13))],
        ),
        (
            "fn g() {\n    let x = id(x: 1)\n}",
            vec![(Code::LabeledArgument, (2, 16))],
        ),
        (
            "fn g() {\n    let c = Cell(value: 1)\n}",
            vec![(Code::MissingField, (2, 13))],
        ),
        (
            "fn g() {\n    let c = Cell(next: None, value: 1)\n}",
            vec![(Code::FieldOutOfOrder, (2, 18))],
        ),
        (
            "fn g() {\n    let t = Tree.Node(Tree.Leaf, 5)\n}",
            vec![(Code::ArgumentCount, (2, 13))],
        ),
    ] {
        codes(&with_generics(line), &expected);
    }
}

/// The new form of `CannotInfer` is at the callee's name, with a help, and so is the list on a
/// variant.
#[test]
fn cannot_infer_should_be_reported_at_the_callee_name() {
    let src = with_generics("fn g() {\n    let x = first([])\n}");
    let d = diagnostic(&src, Code::CannotInfer, (2, 13, "first"), &[]);
    assert!(d.help().is_some());
    let src = with_generics("fn g() {\n    let t = Tree.Leaf\n}");
    diagnostic(&src, Code::CannotInfer, (2, 13, "Tree.Leaf"), &[]);
    let src = with_generics("fn g() -> Tree<i64> { Tree.Node<i64>(Tree.Leaf, 5, Tree.Leaf) }");
    let d = diagnostic(&src, Code::TypeArguments, (1, 32, "<i64>"), &[]);
    assert!(d.help().is_some());
}

/// A type parameter and a built-in type take no type arguments.
#[test]
fn parameters_and_built_ins_should_take_no_type_arguments() {
    codes(
        "fn g<T>(x: T<i64>, y: i64<bool>) {}\nfn main() {}",
        &[
            (Code::TypeArguments, (1, 13)),
            (Code::TypeArguments, (1, 26)),
        ],
    );
}

/// A generic call's arguments in each form reach the report of a non-generic one, and an
/// argument whose type cannot fit its parameter's leaves the parameter to the others.
#[test]
fn generic_arguments_that_do_not_check_should_report_as_other_arguments() {
    let two = "class Two<A, B> {\n    a: A\n    b: B\n}\n";
    for (line, expected) in [
        // A method that takes no type arguments of its own is reported once.
        (
            "fn g(p: Pile<i64>) {\n    p.push<i64>(1)\n}".to_string(),
            vec![(Code::TypeArguments, (2, 11))],
        ),
        // A variant's misplaced list leaves nothing to infer.
        (
            "fn g() {\n    let x = id(Wrap.W<i64>([]))\n}".into(),
            vec![(Code::TypeArguments, (2, 22))],
        ),
        // A unification that fails part way undoes what it bound, which `1` then binds.
        (
            format!(
                "{two}fn same<T>(t: Two<T, T>, x: T) {{}}\nfn g(t: Two<string, bool>) {{\n    same(t, 1)\n}}"
            ),
            vec![(Code::Mismatch, (7, 10))],
        ),
        // A parameter's type that is not found may be what would have bound it.
        (
            "fn f<T>(x: Nope<T>) -> T { panic(\"x\") }\nfn g() {\n    let y = f(1)\n}".into(),
            vec![(Code::UnknownType, (1, 12))],
        ),
        (
            "class Bad<T> {\n    v: Nope<T>\n}\nfn g() {\n    let b = Bad(v: 1)\n}".into(),
            vec![(Code::UnknownType, (2, 8))],
        ),
        // The expected type binds `B`, and the result's `A` is reported against it.
        (
            format!(
                "{two}fn both<A, B>(a: A, b: B) -> Two<A, B> {{ Two(a: a, b: b) }}\nfn g(n: i64) {{\n    let x: Two<f64, [i64]> = both(n, [])\n}}"
            ),
            vec![(Code::Mismatch, (7, 30))],
        ),
        (
            "fn g() {\n    let x = Wrap.W([])\n}".into(),
            vec![(Code::CannotInfer, (2, 13))],
        ),
        // A variant that fits waits for the call around it, which reports.
        (
            "fn g() {\n    let x = id(Wrap.W([]))\n}".into(),
            vec![(Code::CannotInfer, (2, 13))],
        ),
        (
            "fn g() {\n    let x = id({ let y = 1 })\n}".into(),
            vec![(Code::TypeArgument, (2, 16))],
        ),
        (
            "fn bad<T>(x: T, y: Nope) {}\nfn g() {\n    bad(1, 2)\n}".into(),
            vec![(Code::UnknownType, (1, 20))],
        ),
        (
            format!(
                "{two}fn f(a: i64, b: i64, c: i64, d: i64) {{\n    let x = Two(a: a < b, c > (d))\n}}"
            ),
            vec![(Code::TypeArguments, (6, 20))],
        ),
        (
            "fn both<T>(b: Box<T>, x: T) {}\nfn g() {\n    both(Pile<string>(top: None), 1)\n}"
                .into(),
            vec![(Code::Mismatch, (3, 10))],
        ),
        (
            "fn both<T>(o: Opt2<T>, x: T) {}\nfn g() {\n    both(Wrap.W(\"a\"), 1)\n}".into(),
            vec![(Code::Mismatch, (3, 10))],
        ),
        // A labeled variant is an error, and a missing name too; a bound one is typed.
        (
            "fn g() {\n    let n: i64 = Wrap.W(x: 1)\n}".into(),
            vec![(Code::LabeledArgument, (2, 25))],
        ),
        (
            "fn g() {\n    let n: i64 = Tree.Node(Tree<i64>.Leaf, nope, Tree.Leaf)\n}".into(),
            vec![(Code::Mismatch, (2, 18)), (Code::BindingNotFound, (2, 44))],
        ),
        (
            "fn g() {\n    let n: i64 = Cell(value: 1, next: nope)\n}".into(),
            vec![(Code::Mismatch, (2, 18)), (Code::BindingNotFound, (2, 39))],
        ),
        (
            "enum Mix<T> { M(x: T, T) }\nfn g() {\n    let m = Mix.M(x: 1, 2)\n}".into(),
            vec![(Code::MixedLabels, (1, 23))],
        ),
        (
            "enum Lab<T> { P(x: T, y: T) }\nfn g() {\n    let p = Lab.P(y: 1, x: 2)\n}".into(),
            vec![(Code::FieldOutOfOrder, (3, 19))],
        ),
        (
            "fn g() {\n    let x = E.map<string>(\"a\")\n}".into(),
            vec![(Code::MemberKind, (2, 15))],
        ),
    ] {
        codes(&with_generics(&line), &expected);
    }
}

/// A generic method's call that cannot check reports once, whether it waits for context or
/// its receiver's type is not known.
#[test]
fn a_generic_method_call_that_does_not_check_should_report_once() {
    for (line, expected) in [
        // The generic method waits for `id`, which nothing binds.
        (
            "fn g(b: Box<i64>) {\n    let x = id(b.map_all([]))\n}".to_string(),
            vec![(Code::CannotInfer, (2, 13))],
        ),
        // The statements before a waiting block's value are checked, even when nothing binds.
        (
            "fn g(b: Box<i64>) {\n    let x = id({ let n = nope; b.make() })\n}".into(),
            vec![
                (Code::CannotInfer, (2, 13)),
                (Code::BindingNotFound, (2, 26)),
            ],
        ),
        // A receiver whose type is not known waits for nothing, and reports its own error.
        (
            "fn g() {\n    let xs: [i64] = id(nope.b.make())\n}".into(),
            vec![(Code::BindingNotFound, (2, 24))],
        ),
        (
            "fn g(n: i64) {\n    let xs: [i64] = id(n.b.make())\n}".into(),
            vec![(Code::NoMember, (2, 26))],
        ),
        (
            "fn g(b: Box<i64>) {\n    let xs: [i64] = id(b.nope.make())\n}".into(),
            vec![(Code::NoMember, (2, 26))],
        ),
        (
            "fn g(b: Box<i64>) {\n    let xs: [i64] = id(b.nope())\n}".into(),
            vec![(Code::NoMember, (2, 26))],
        ),
        (
            "fn g() {\n    let xs: [i64] = id(nope.make())\n}".into(),
            vec![(Code::BindingNotFound, (2, 24))],
        ),
        // Each call's ranking finds the receiver checked already.
        (
            "fn g() {\n    let xs: [i64] = id(id(nope.b.make()))\n}".into(),
            vec![(Code::BindingNotFound, (2, 27))],
        ),
    ] {
        codes(&with_generics(&line), &expected);
    }
}

/// Ranking an argument checks a method's receiver, and the statements before a block's value,
/// once: what checking the argument reports, it reports once.
#[test]
fn work_done_ahead_should_not_be_done_again() {
    for (line, expected) in [
        (
            "fn g(b: Box<i64>) {\n    let xs: [i64] = id({ let n = nope; b.make() })\n}",
            vec![(Code::BindingNotFound, (2, 34))],
        ),
        (
            "fn g(b: Box<i64>) {\n    let xs: [[i64]] = id([{ let n = nope; b.make() }])\n}",
            vec![(Code::BindingNotFound, (2, 37))],
        ),
        (
            "fn g(b: Box<i64>) {\n    let xs: [i64] = id(id({ let n = nope; b.make() }))\n}",
            vec![(Code::BindingNotFound, (2, 37))],
        ),
        (
            "fn g(b: Box<i64>, c: bool) {\n    let xs: [i64] = id(if c { let n = nope; b.make() } else { let m = 1; b.make() })\n}",
            vec![(Code::BindingNotFound, (2, 39))],
        ),
        // A join checked again checks again what its first check ranked ahead.
        (
            "class Bx {\n    fn make<U>(self) -> [U] { [] }\n}\nfn f<T>(x: T) -> string { \"\" }\nfn g() {\n    let n: u8 = f(({ nope; Bx() }).make()) + \"x\"\n}",
            vec![
                (Code::CannotInfer, (6, 17)),
                (Code::BindingNotFound, (6, 22)),
            ],
        ),
    ] {
        codes(&with_generics(line), &expected);
    }
}

/// A path before the `.` is not checked as a value, so a call on it does not wait.
#[test]
fn calls_on_paths_should_not_wait() {
    for (line, expected) in [
        (
            "fn g() {\n    let x = id(math.sqrt(2.0))\n}",
            vec![(Code::MathIdiom, (2, 16))],
        ),
        (
            "fn g() {\n    let t = id(Tree<i64>.Node(Tree.Leaf, 1, Tree.Leaf))\n}",
            vec![],
        ),
    ] {
        codes(&with_generics(line), &expected);
    }
}

/// An argument that would wait for context, with a call whose shape or declaration is wrong,
/// reports that, not the enclosing call's inference: it is checked with context when there is
/// some, else on its own.
#[test]
fn waiting_calls_should_report_their_own_shape() {
    for (line, expected) in [
        // A wrong count, or a label, of a function's, a method's, a class's or a variant's.
        (
            "fn g() {\n    let x = id(first())\n}",
            vec![(Code::ArgumentCount, (2, 16))],
        ),
        (
            "fn g(b: Box<i64>) {\n    let x = id(b.map_all())\n}",
            vec![(Code::ArgumentCount, (2, 16))],
        ),
        (
            "fn g() {\n    let x = id(Box())\n}",
            vec![(Code::MissingField, (2, 16))],
        ),
        (
            "fn g() {\n    let x = id(Wrap.W())\n}",
            vec![(Code::ArgumentCount, (2, 16))],
        ),
        // With context, a wrong shape reports only itself.
        (
            "fn g() {\n    let x: [i64]? = id(first(xs: []))\n}",
            vec![(Code::LabeledArgument, (2, 30))],
        ),
        (
            "fn g(b: Box<i64>) {\n    let xs: [[i64]] = id(b.map_all(u: []))\n}",
            vec![(Code::LabeledArgument, (2, 36))],
        ),
        (
            "fn g() {\n    let w: Wrap<[i64]> = id(Wrap.W(x: []))\n}",
            vec![(Code::LabeledArgument, (2, 36))],
        ),
        (
            "enum Tag<T> { P(x: [T]) }\nfn g() {\n    let p: Tag<i64> = id(Tag.P(y: []))\n}",
            vec![(Code::NoMember, (3, 32))],
        ),
        (
            "class Two<A, B> {\n    a: A\n    b: B\n}\nfn g() {\n    let t: Two<[i64], [i64]> = id(Two(b: [], a: []))\n}",
            vec![(Code::FieldOutOfOrder, (6, 39))],
        ),
        (
            "class Two<A, B> {\n    a: A\n    b: B\n}\nfn g() {\n    let t = id(Two(b: [], a: []))\n}",
            vec![(Code::FieldOutOfOrder, (6, 20))],
        ),
        // Without, a labeled argument is still checked, as in the call alone.
        (
            "fn g() {\n    let x = id(first(xs: []))\n}",
            vec![
                (Code::CannotInfer, (2, 16)),
                (Code::LabeledArgument, (2, 22)),
            ],
        ),
        (
            "fn g(b: Box<i64>) {\n    let x = id(b.map_all(u: []))\n}",
            vec![
                (Code::CannotInfer, (2, 18)),
                (Code::LabeledArgument, (2, 26)),
            ],
        ),
        (
            "fn g() {\n    let x = id(Box(w: []))\n}",
            vec![(Code::NoMember, (2, 20))],
        ),
        (
            "fn g() {\n    let t = id(Tree.Leaf())\n}",
            vec![(Code::VariantForm, (2, 25))],
        ),
        (
            "enum Mix<T> { M(x: T, T) }\nfn g() {\n    let m = id(Mix.M(x: [], []))\n}",
            vec![(Code::MixedLabels, (1, 23))],
        ),
        // A parameter's or the return type that is not found poisons the call.
        (
            "fn f<T>(x: Nope<T>) -> [T] { [] }\nfn g() {\n    let y = id(f([]))\n}",
            vec![(Code::UnknownType, (1, 12))],
        ),
        (
            "fn f<T>(x: [T]) -> Nope<T> {}\nfn g() {\n    let y = id(f([]))\n}",
            vec![(Code::UnknownType, (1, 20))],
        ),
        (
            "fn f<T>(x: [T]) -> Nope<T> {}\nfn g() {\n    let y = f([])\n}",
            vec![(Code::UnknownType, (1, 20))],
        ),
        (
            "class Bin {\n    fn put<U>(self, x: Nope<U>) -> [U] { [] }\n}\nfn g(b: Bin) {\n    let x = id(b.put([]))\n}",
            vec![(Code::UnknownType, (2, 24))],
        ),
        (
            "class Bin {\n    fn put<U>(self, x: [U]) -> Nope<U> {}\n}\nfn g(b: Bin) {\n    let x = id(b.put([]))\n}",
            vec![(Code::UnknownType, (2, 32))],
        ),
        (
            "class Bad<T> {\n    v: Nope<T>\n}\nfn g() {\n    let b = id(Bad(v: []))\n}",
            vec![(Code::UnknownType, (2, 8))],
        ),
        (
            "enum Bad<T> { P(Nope<T>) }\nfn g() {\n    let b = id(Bad.P([]))\n}",
            vec![(Code::UnknownType, (1, 17))],
        ),
    ] {
        codes(&with_generics(line), &expected);
    }
}

/// An instantiation that gives a declaration a type argument that grows, on a cycle back to that
/// declaration, is one error per cycle: at the first growing instantiation, with a note at each
/// other instantiation of the shortest way back. A cycle whose arguments do not grow, and a
/// growing one that does not come back, are clean, and so is a class whose method makes a
/// bigger instance of it.
#[test]
fn growing_instantiation_cycles_should_be_one_error_each() {
    for (src, at, notes) in [
        ("fn f<T>(x: T) { f([x]) }", (1, 17), vec![]),
        (
            "fn f<T>(x: T) { g([x]) }\nfn g<T>(x: T) { f(x) }",
            (1, 17),
            vec![(2, 17)],
        ),
        ("enum Nest<T> { A(Nest<[T]>)\n B }", (1, 18), vec![]),
        ("class C<T> { next: C<[T]>? }", (1, 20), vec![]),
        ("fn f<T>(x: T) { let y: [T] = [x]\n f(y) }", (2, 2), vec![]),
        // A method is its own declaration, its class's parameters first.
        (
            "class C<T> {\n    v: T\n    fn m(self) { C(v: [self.v]).m() }\n}",
            (3, 18),
            vec![],
        ),
        // A call inside an assigned place.
        (
            "fn f<T>(x: T) -> i64 {\n    let mut xs = [0]\n    xs[f([x])] = 1\n    0\n}",
            (3, 8),
            vec![],
        ),
        // `U` grows into `T` and `T` comes back into `U` at the same call, noted once.
        ("fn f<T, U>(x: T, y: U) { f(y, [x]) }", (1, 26), vec![]),
        // Each parameter grows into itself at one call, reported once.
        ("fn f<T, U>(x: T, y: U) { f([x], [y]) }", (1, 26), vec![]),
        ("class P<A, B> { next: P<[A], [B]>? }", (1, 23), vec![]),
        (
            "class Cell<T> {\n    v: T\n    fn spread<U>(self, x: U) { Cell(v: [self.v]).spread([x]) }\n}",
            (3, 32),
            vec![],
        ),
        // Two growing edges, one error, at the first in source order: the payload, not the
        // field, though classes are numbered first.
        (
            "enum E<T> { A(C<[T]>)\n B }\nclass C<T> {\n    e: E<[T]>?\n}",
            (1, 15),
            vec![(4, 8)],
        ),
        // The way back is the shortest: `g` calls `f` directly as well as through `h`.
        (
            "fn f<T>(x: T) { g([x]) }\nfn g<T>(x: T) {\n    h(x)\n    f(x)\n}\nfn h<T>(x: T) { f(x) }",
            (1, 17),
            vec![(4, 5)],
        ),
        // Shortest by calls, however deep the arguments on the way: `f([[[[x]]]])` back from
        // `g` directly, not through `h`, before it and after it.
        (
            "fn f<T>(x: T) { g([x]) }\nfn g<T>(x: T) {\n    f([[[[x]]]])\n    h(x)\n}\nfn h<T>(x: T) { f(x) }",
            (1, 17),
            vec![(3, 5)],
        ),
        (
            "fn f<T>(x: T) { g([x]) }\nfn g<T>(x: T) {\n    h(x)\n    f([[[x]]])\n}\nfn h<T>(x: T) { f(x) }",
            (1, 17),
            vec![(4, 5)],
        ),
        // An argument mentions a parameter inside a class, an enum or an optional, and a
        // second class or enum is numbered after the first.
        (
            "class B<T> { v: T }\nclass C<T> { next: C<B<T>>? }",
            (2, 20),
            vec![],
        ),
        (
            "enum E<T> { A(T)\n B }\nenum N<T> { A(N<E<T>>)\n B }",
            (3, 15),
            vec![],
        ),
        ("class C<T> { next: C<[T?]>? }", (1, 20), vec![]),
        // Calls inside a guard, a loop and an appended value.
        (
            "fn f<T>(x: T) -> bool {\n    match 1 {\n        n if f([x]) => true\n        _ => false\n    }\n}",
            (3, 14),
            vec![],
        ),
        // An enum's method, its enum's parameters first.
        (
            "enum E<T> {\n    A(T)\n    B\n    fn m(self, x: T) { E.A([x]).m([x]) }\n}",
            (4, 24),
            vec![],
        ),
        (
            "fn f<T>(x: T) {\n    loop {\n        f([x])\n    }\n}",
            (3, 9),
            vec![],
        ),
        (
            "fn f<T>(x: T) -> string {\n    let mut xs = [\"\"]\n    xs[0] = xs[0] + f([x])\n    xs[0]\n}",
            (3, 21),
            vec![],
        ),
    ] {
        assert_eq!(
            found(&format!("{src}\nfn main() {{}}")),
            [(Code::InstantiationCycle, at, notes)],
            "{src}"
        );
    }
    for src in [
        "fn f<T>(x: T, n: i64) { if n > 0 { f(x, n - 1) } }",
        "fn f<T>(x: T) { g(x) }\nfn g<T>(x: T) { f(x) }",
        "class List<T> { next: List<T>? }",
        "enum Tree<T> { Leaf\n Node(Tree<T>, T, Tree<T>) }",
        "fn f<T>(x: T) { g([x]) }\nfn g<T>(x: T) {}",
        "class C<T> { v: T\n fn up(self) -> C<[T]> { C(v: [self.v]) } }\nfn g() -> C<[i64]> { C(v: 1).up() }",
        "fn f<T, U>(x: T, y: U, n: i64) { if n > 0 { f(y, x, n - 1) } }",
    ] {
        clean(&format!("{src}\nfn main() {{}}"));
    }
}

/// Instantiation cycles beside each other and beside other errors.
#[test]
fn instantiation_cycles_should_be_reported_beside_other_errors() {
    // Each parameter grows into itself at a call of its own, reported at each.
    codes(
        "fn f<T, U>(x: T, y: U) {\n    f([x], y)\n    f(x, [y])\n}\nfn main() {}",
        &[
            (Code::InstantiationCycle, (2, 5)),
            (Code::InstantiationCycle, (3, 5)),
        ],
    );
    // A declaration no value can be built of has that error alone.
    codes(
        "enum Nest<T> { A(Nest<[T]>) }\nfn main() {}",
        &[(Code::NeverConstructed, (1, 6))],
    );
    codes(
        "class C<T> {\n    next: C<[T]>\n}\nfn main() {}",
        &[(Code::NeverConstructed, (2, 5))],
    );
    // A class that cannot be built only through another's error keeps its own cycle.
    codes(
        "enum Bad { A(Bad) }\nclass C<T> {\n    b: Bad\n    next: C<[T]>?\n}\nfn main() {}",
        &[
            (Code::NeverConstructed, (1, 6)),
            (Code::InstantiationCycle, (4, 11)),
        ],
    );
}

/// `tir` writes a call's type arguments as a program writes them, a class by its name.
#[test]
fn call_type_arguments_should_name_their_classes_in_tir() {
    contains(
        "class Box<T> {\n    v: T\n}\nfn id<T>(x: T) -> T { x }\nfn main() {\n    print(id(Box(v: 1)).v)\n}",
        &["(call id<Box<i64>> "],
    );
}

/// Parameters of array types whose elements are optional, and `main`.
const OPTIONAL_ELEMENTS: &str = "fn g1<T>(xs: [T?]) -> i64 { 0 }\nfn g3<T>(xs: [[T]]) -> i64 { 0 }\nfn g4<T>(xs: [[T?]]) -> i64 { 0 }\nfn g5<T>(xs: [T]) -> i64 { 0 }\nfn main() {}";

/// `body` in a function of `b: u8`, `x: i64` and `ys: [i64]`, on lines 2 on, then
/// `OPTIONAL_ELEMENTS`.
fn with_optional_elements(body: &str) -> String {
    format!("fn h(b: u8, x: i64, ys: [i64]) -> i64 {{\n    {body}\n}}\n{OPTIONAL_ELEMENTS}")
}

/// An array literal checked against a parameter that names an unbound type parameter in an
/// optional element binds it from the elements, and wraps the ones that need it.
#[test]
fn generic_array_literals_should_take_optional_elements() {
    for (call, callee, expected) in [
        ("g1([1, 2])", "g1", "i64"),
        ("g1([[1]])", "g1", "[i64]"),
        ("g1([1, None])", "g1", "i64"),
        ("g1([None, 1])", "g1", "i64"),
        // A literal takes the type of the concrete element, whichever comes first.
        ("g1([b, 1])", "g1", "u8"),
        ("g1([1, b])", "g1", "u8"),
        ("g3([[], [1]])", "g3", "i64"),
        ("g4([[1]])", "g4", "i64"),
        ("g4([[None, 1]])", "g4", "i64"),
        ("g4([[], [1]])", "g4", "i64"),
        // A parameter that is no array of optionals keeps the join.
        ("g5([1, None])", "g5", "i64?"),
    ] {
        let found = call_types(&with_optional_elements(call), callee);
        assert_eq!(found, [expected, "i64"], "{call}");
    }
}

/// A mistake in an array literal that takes optional elements is reported once, where it is.
#[test]
fn generic_array_literals_should_report_each_mistake_once() {
    for (body, expected) in [
        ("g1([None, None])", vec![(Code::CannotInfer, (2, 5))]),
        ("g1([nope, None])", vec![(Code::BindingNotFound, (2, 9))]),
        (
            "g1([nope, None, []])",
            vec![(Code::BindingNotFound, (2, 9))],
        ),
        ("g1([b, x])", vec![(Code::Mismatch, (2, 12))]),
        // An array is invariant: a variable of `[i64]` is not a `[i64?]`.
        (
            "let zs = [1, 2]\n    g1(zs)",
            vec![(Code::Mismatch, (3, 8))],
        ),
        ("g1(ys)", vec![(Code::Mismatch, (2, 8))]),
        // An element with no value, `unit` or `never`, is reported where it is, binds nothing,
        // and leaves a `None` beside it silent.
        ("g1([print(1), 2])", vec![(Code::NoValue, (2, 9))]),
        ("g1([2, print(1)])", vec![(Code::NoValue, (2, 12))]),
        (
            "g1([print(1), print(2)])",
            vec![(Code::NoValue, (2, 9)), (Code::NoValue, (2, 19))],
        ),
        ("g1([print(1), None])", vec![(Code::NoValue, (2, 9))]),
        ("g4([print(1)])", vec![(Code::NoValue, (2, 9))]),
        (
            "g1([if x > 0 { return 0 } else { return 1 }, 1])",
            vec![(Code::NoValue, (2, 9))],
        ),
        (
            "g1([1, if x > 0 { return 0 } else { return 1 }])",
            vec![(Code::NoValue, (2, 12))],
        ),
        // Nothing in the outer literal binds `T`, so the first `None` is the one reported.
        ("g4([[None], [1]])", vec![(Code::CannotInfer, (2, 10))]),
    ] {
        codes(&with_optional_elements(body), &expected);
    }
}
