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
        // Only `Option.Some` is `Some`: a method of that name gives its own type.
        "class C {\n    n: i64\n    fn Some(self, _x: i64?) -> i64 { self.n }\n}\nfn f(c: C) -> i64 {\n    let r = id(c.Some(None))\n    r\n}\nfn id<T>(x: T) -> T { x }",
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
    with_prelude();
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
    with_prelude();
    let decls = MISREAD_DECLS;
    for (line, expected) in [
        // A method of the prelude's `extend i64` is a number's method too.
        (
            "fn f(a: i64, b: i64) -> bool { h(a.max < b, b > (1)) }",
            vec![(Code::TypeArguments, (1, 34))],
        ),
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
    with_prelude();
    let lacks = |at| vec![(Code::ParamLacks, at)];
    for (src, expected) in [
        ("fn f<T>(a: T, b: T) -> bool { a < b }", lacks((1, 31))),
        ("fn f<T>(a: T, b: T) -> bool { a == b }", lacks((1, 31))),
        ("fn f<T>(a: T, b: T) -> bool { a != b }", lacks((1, 31))),
        ("fn f<T>(a: T, b: T) -> bool { a + b }", lacks((1, 31))),
        ("fn f<T>(a: T, b: T) -> bool { -a }", lacks((1, 32))),
        ("fn f<T>(a: T, b: T) -> bool { [a] == [b] }", lacks((1, 31))),
        (
            "fn f<T>(a: T, b: i64) -> bool { b < a }",
            vec![(Code::Mismatch, (1, 37))],
        ),
        (
            "fn f<T>(xs: [T], a: T) -> bool { xs.contains(a) }",
            vec![(Code::NotConform, (1, 34))],
        ),
        (
            "fn f<T>(a: T, b: T) -> [T] { [a; 2] }",
            vec![(Code::SameObject, (1, 31))],
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

/// An optional expected type binds a type parameter whole only for an argument that waits
/// for it. Else it binds the payload: `y ?? id(2)` gives the `P` that `y ?? 2` gives.
#[test]
fn an_optional_expected_type_should_bind_the_payload_unless_an_argument_waits() {
    infers(&[
        (
            "fn g() {\n    let x: i64? = id(5)\n}",
            "id",
            &["i64", "i64"],
        ),
        ("fn g(y: i64?) -> i64 { y ?? id(2) }", "id", &["i64", "i64"]),
        (
            "fn g(y: i64?, c: bool) -> i64 { y ?? (if c { id(2) } else { 3 }) }",
            "id",
            &["i64", "i64"],
        ),
        (
            "fn g(y: i64?) -> i64? { y ?? id(None) }",
            "id",
            &["i64?", "i64?"],
        ),
        (
            "fn pick<T>(a: T, b: T) -> T { a }\nfn g(y: i64?) -> i64? { y ?? pick(5, None) }",
            "pick",
            &["i64?", "i64?"],
        ),
    ]);
}

/// A generic call whose arguments cannot bind one of its type parameters needs its context,
/// whatever its arguments are: the context of an outer generic call fixes the parameter.
#[test]
fn a_call_whose_arguments_cannot_bind_a_parameter_should_wait_for_context() {
    let pair = "class Tag<T, U> {\n    a: T\n}\n";
    let bag = "class Bag<T> {\n    v: T\n    fn conv<U>(self, n: i64) -> [U] { [] }\n}\n";
    infers(&[
        // A function whose return type alone names a parameter.
        (
            "fn conv<T, U>(x: T) -> [U] { [] }\nfn g() {\n    let r: [string] = id(conv(1))\n}",
            "conv",
            &["i64", "string", "[string]"],
        ),
        // A class whose fields leave a parameter unnamed.
        (
            &format!("{pair}fn g() {{\n    let r: Tag<i64, string> = id(Tag(a: 1))\n}}"),
            "Tag",
            &["i64", "string", "Tag<i64, string>"],
        ),
        // A method names its owner's parameters through its receiver, not its own.
        (
            &format!("{bag}fn g(b: Bag<i64>) {{\n    let r: [string] = id(b.conv(1))\n}}"),
            "Bag.conv",
            &["i64", "string", "[string]"],
        ),
        // A variant of a generic enum, which names one of its two parameters.
        (
            "enum R<T, E> {\n    O(T)\n    X(E)\n}\nfn g() {\n    let r: R<i64, string> = id(R.O(1))\n}",
            "R",
            &["i64", "string", "R<i64, string>"],
        ),
    ]);
    clean(&with_decls(
        "enum R<T, E> {\n    O(T)\n    X(E)\n}\nfn g() {\n    let r: R<R<i64, string>, string> = R.O(R.X(\"x\"))\n}",
    ));
}

/// The context of an outer call binds only what the type of the inner call names: a type
/// parameter that no argument and not the type of the call names stays unbound, and is reported
/// at the inner call, not at the outer one.
#[test]
fn an_unnamed_parameter_should_be_reported_at_its_own_call() {
    let keep = "class Bag<T> {\n    v: T\n    fn keep<U, V>(self, u: U) -> U { u }\n}\n";
    for (src, expected) in [
        (
            "fn f<T, U>(x: T) -> T { x }\nfn g() {\n    let r = id(f(1))\n}".to_string(),
            vec![(Code::CannotInfer, (3, 16))],
        ),
        (
            format!("{keep}fn g(b: Bag<i64>) {{\n    let s = id(b.keep(\"a\"))\n}}"),
            vec![(Code::CannotInfer, (6, 18))],
        ),
        // The duplicate is reported once: the second `T` is a name nothing uses.
        (
            "fn f<T, T>(x: T) -> T { x }\nfn g() {\n    let r = id(f(1))\n}".to_string(),
            vec![(Code::DefinedTwice, (1, 9))],
        ),
        // A parameter that only the return type names is open, but `U` is not: `f` reports it.
        (
            "fn f<T, U, V>(x: T) -> [V] { [] }\nfn g() {\n    let r = id(f(1))\n}".to_string(),
            vec![(Code::CannotInfer, (3, 16))],
        ),
        // A broken name is skipped, so that the other `T`, which only the return type names,
        // still waits for its context.
        (
            "fn f<T, T>(x: i64) -> T { todo }\nfn g() {\n    let r: i64 = id(f(1))\n}".to_string(),
            vec![(Code::DefinedTwice, (1, 9)), (Code::TodoLeft, (1, 27))],
        ),
        // A broken name in a class, an enum or a built-in name is reported once, at its
        // declaration, and no call waits for it.
        (
            "class Tag<T, T> {\n    a: T\n}\nfn g() {\n    let r = id(Tag(a: 1))\n}".to_string(),
            vec![(Code::DefinedTwice, (1, 14))],
        ),
        (
            "enum Pick<T, T> {\n    O(T)\n}\nfn g() {\n    let r = id(Pick.O(1))\n}".to_string(),
            vec![(Code::DefinedTwice, (1, 14))],
        ),
        (
            "class Tag<T, i64> {\n    a: T\n}\nfn g() {\n    let r = id(Tag(a: 1))\n}".to_string(),
            vec![(Code::BuiltInName, (1, 14))],
        ),
    ] {
        codes(&with_generics(&src), &expected);
    }
}

/// A method names its owner's parameters through its receiver, and `Option.Some` is `Some`: an
/// outer call needs no context for either.
#[test]
fn a_call_that_binds_its_parameters_should_not_wait_for_context() {
    clean(&with_generics(
        "class Bag<T> {\n    v: T\n    fn keep<U>(self, u: U) -> T { self.v }\n}\nfn g(b: Bag<i64>) {\n    let s = id(b.keep(\"a\"))\n}",
    ));
    clean(&with_generics(
        "fn g() {\n    let b = id(Option.Some(1))\n}",
    ));
}

/// Finding the type parameters that a call's parameter types name visits each type once, so it
/// grows with the declaration, not with the square of its type parameters.
#[test]
fn named_parameters_should_be_found_in_one_walk() {
    let marked = |n: usize| {
        let types: Vec<_> = (0..n).map(|i| format!("T{i}")).collect();
        let params: Vec<_> = (0..n).map(|i| format!("x{i}: T{i}")).collect();
        let src = format!(
            "fn id<T>(x: T) -> T {{ x }}\nfn f<{}>({}) -> T0 {{ x0 }}\nfn main() {{\n    let r: i64 = id(f({}))\n}}",
            types.join(", "),
            params.join(", "),
            vec!["1"; n].join(", "),
        );
        generic::MARKED.set(0);
        clean(&src);
        generic::MARKED.get()
    };
    assert_near_linear(marked(100), marked(200), marked(300));
    // A type without a parameter is not walked again at each call: the declaration and the
    // number of calls grow together.
    let repeated = |n: usize| {
        let types: Vec<_> = (0..n).map(|i| format!("P{i}")).collect();
        let src = format!(
            "fn id<T>(x: T) -> T {{ x }}\nclass C<{}> {{}}\nfn f<T>(x: T, c: C<{}>) -> T {{ x }}\nfn g(c: C<{}>) {{\n{}}}\nfn main() {{}}",
            types.join(", "),
            vec!["i64"; n].join(", "),
            vec!["i64"; n].join(", "),
            "    let _ = id(f(1, c))\n".repeat(n),
        );
        generic::MARKED.set(0);
        clean(&src);
        generic::MARKED.get()
    };
    assert_near_linear(repeated(100), repeated(200), repeated(300));
    // A call that does not fit its declaration is not walked at all: a function, a method, a
    // class and a variant.
    let src = format!(
        "fn id<T>(x: T) -> T {{ x }}\nfn f<T>(x: T) -> T {{ x }}\nclass Bag<T> {{\n    v: T\n    fn keep<U>(self, u: U) -> T {{ self.v }}\n}}\nenum Pick<T> {{ O(T) }}\nfn main(b: Bag<i64>) {{\n{}}}",
        "    let r: i64 = id(f())\n    let s: i64 = id(b.keep())\n    let t: Bag<i64> = id(Bag())\n    let u: Pick<i64> = id(Pick.O())\n".repeat(20),
    );
    generic::MARKED.set(0);
    checked(&src);
    assert_eq!(generic::MARKED.get(), 0);
    // `Ok` and `Err`, which the prelude declares, too. Its `extend i64` names its target in a
    // walk of its own, which a program with no call adds as well.
    with_prelude();
    generic::MARKED.set(0);
    checked("fn main() {}");
    let blocks = generic::MARKED.get();
    let src = format!(
        "fn id<T>(x: T) -> T {{ x }}\nfn main() {{\n{}}}",
        "    let r: Result<i64, string> = id(Ok())\n".repeat(20),
    );
    generic::MARKED.set(0);
    checked(&src);
    assert_eq!(generic::MARKED.get(), blocks);
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

/// A call whose `n` parameters each have a bound, and whose arguments bind them one by one. Each
/// binding checks its own bound, so the substitution list that the check reads is built once for
/// the call and changes one entry for each binding. A list built again for each binding makes the
/// work grow with the square of `n`.
#[test]
fn early_bound_checks_should_build_the_substitution_list_once() {
    let listed = |n: usize| {
        let types: Vec<_> = (0..n).map(|i| format!("T{i}: I")).collect();
        let params: Vec<_> = (0..n).map(|i| format!("_a{i}: T{i}")).collect();
        let src = format!(
            "interface I {{\n}}\nclass W: I {{\n}}\n\
             fn h<{}>({}) {{}}\nfn main() {{\n    let w = W()\n    h({})\n}}",
            types.join(", "),
            params.join(", "),
            vec!["w"; n].join(", ")
        );
        generic::LISTED.set(0);
        clean(&src);
        generic::LISTED.get()
    };
    assert!(listed(1) > 0, "the counter should count the list");
    assert_near_linear(listed(100), listed(200), listed(300));
}

/// A call whose `n` parameters each have a bound `K<Z>`, which waits until the last argument binds
/// `Z`. Each waiting bound moves once to the list of `Z` and is checked once. Taking the whole list
/// of waiting bounds for each binding makes the work grow with the square of `n`.
#[test]
fn waiting_bounds_should_be_looked_at_once_for_each_parameter_they_name() {
    let examined = |n: usize| {
        let types: Vec<_> = (0..n).map(|i| format!("T{i}: K<Z>")).collect();
        let params: Vec<_> = (0..n).map(|i| format!("_a{i}: T{i}")).collect();
        let src = format!(
            "interface K<X> {{\n}}\nclass V: K<i64> {{\n}}\n\
             fn h<Z, {}>({}, _z: Z) {{}}\nfn main() {{\n    let v = V()\n    h({}, 1)\n}}",
            types.join(", "),
            params.join(", "),
            vec!["v"; n].join(", ")
        );
        generic::EXAMINED.set(0);
        clean(&src);
        generic::EXAMINED.get()
    };
    assert!(examined(1) > 0, "the counter should count the bounds");
    assert_near_linear(examined(100), examined(200), examined(300));
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
    assert!(one > 0, "the counter should count the walk of `known`");
    assert_eq!(walked(&vec!["x"; 100].join(", ")), (one, 0));
    let (bad, _) = walked("b, x");
    assert_eq!(walked(&format!("{}x", "b, ".repeat(100))), (bad, 100));
}

/// Each waiting argument is a generic call for a parameter of its own, which its context binds.
/// The sweeps look at each parameter type a fixed number of times. So the work grows with the
/// arguments, not with their square.
#[test]
fn waiting_arguments_should_cost_the_same_walk_each() {
    with_prelude();
    let walked = |n: usize| {
        let types: Vec<_> = (0..n).map(|i| format!("T{i}")).collect();
        let params: Vec<_> = (0..n)
            .map(|i| format!("x{i}: Result<T{i}, string>"))
            .collect();
        let src = format!(
            "fn f<{}>({}) {{}}\nfn main() {{ f({}) }}",
            types.join(", "),
            params.join(", "),
            vec!["Ok(1)"; n].join(", ")
        );
        generic::WALKED.set(0);
        clean(&src);
        generic::WALKED.get()
    };
    let fifty = walked(50);
    assert!(fifty > 0, "the counter should count the walks");
    assert_eq!(walked(100), 2 * fifty);
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
    assert_eq!(steps(500), 1502 + LANG_PARAM_STEPS);
    assert_eq!(steps(1000), 3002 + LANG_PARAM_STEPS);
}

/// Work kept ahead of a call is found by its span in one step, so finding it grows with the
/// call's arguments, not with their square: one step per receiver and per block.
#[test]
fn work_kept_ahead_should_be_found_in_one_step() {
    // The declarations hold one `xs.len() == 0`: the operator ranks its sides, which finds the
    // receiver of `len` once.
    const DECLARED: usize = 1;
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
        assert_eq!(inspected(64, arg, ty), 64 + DECLARED, "{arg}");
        assert_eq!(inspected(128, arg, ty), 128 + DECLARED, "{arg}");
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

/// The generic functions that the rows of the two tests below call.
const TRANSPARENT_DECLS: &str = "fn id<T>(x: T) -> T { x }\n\
    fn pick<T>(a: T, b: T) -> T { a }\n\
    enum Two<A, B> { P(A, B) }\n\
    fn mk<A, B>(a: A, b: B) -> Two<A, B> { Two.P(a, b) }\n\
    fn opt<T>(x: T) -> T? { Some(x) }\n\
    fn none_of<T>(x: i64) -> T? { None }\n\
    fn bound<T: Ord>(a: T?, b: T) -> T { b }\n";

/// A value that passes its context to its parts (`dbg`, a block, an `if`, a `match`, a generic
/// call, an array literal) ranks as its parts do, so an integer literal under it is typed by the
/// call that holds it. Each row is one statement of a `main` that has `c: bool` and `x: u8`.
#[test]
fn literals_under_a_transparent_form_should_take_the_type_of_the_call() {
    // `Result`, `Ok` and `dbg` are the prelude's.
    with_prelude();
    for row in [
        "let r: Result<[u8], bool> = Ok(dbg([1]))",
        "let r: Result<bool, [u8]> = Err(({ [1] }))",
        "let r: Result<u8, bool> = Ok(dbg(1))",
        "let r: Result<u8, bool> = Ok({ 1 })",
        "let r: Result<u8, bool> = Ok(if c { 1 } else { 2 })",
        "let r: Result<u8, bool> = Ok(match c {\n true => 1\n false => 2\n })",
        "let r: Result<[u8], bool> = Ok(if c { [1] } else { [2] })",
        "let r: Result<[u8], bool> = Ok([dbg(1)])",
        "let r: Result<[u8], bool> = Ok([{ 1 }])",
        "let r: u8 = id(dbg(1))",
        "let r: u8 = id({ 1 })",
        "let r: u8 = id(if c { 1 } else { 2 })",
        "let r: [u8] = id(dbg([1]))",
        "let r: u8 = id(id(1))",
        "let r: u8?? = Some(Some(1))",
        "let r: Result<u8?, bool> = Ok(Some(1))",
        "let r: Result<u8??, bool> = Ok(Some(Some(1)))",
        "let r: Result<u8?, bool> = Ok(if c { 1 } else { None })",
        // A literal under a call that makes an optional meets the whole optional context.
        "let r: u8? = id(Some(1))",
        "let r: u8? = id(dbg(Some(1)))",
        "let r: u8? = id({ Some(1) })",
        "let r: u8? = id(if c { Some(1) } else { None })",
        "let r: u8? = id(if c { 1 } else { None })",
        "let r: u8? = id(id(Some(1)))",
        "let r: u8? = id(match c {\n true => Some(1)\n false => None\n })",
        "let y: u8? = None\n    let r: u8? = id(if let v = y { print(v)\n Some(1) } else { None })",
        // A generic callee whose declared return type is an optional makes one, as `Some` does.
        "let r: u8? = id(opt(1))",
        "let r: u8? = id(if c { 1 } else { none_of(5) })",
        "let r: u8? = id(match c {\n true => 1\n false => none_of(5)\n })",
        // Only an argument of the bare parameter counts: `T?` takes the payload of `Some(1)`.
        "let r: u8? = bound(Some(1), 2)",
        // A literal alone meets the payload, and the call wraps it.
        "let r: u8? = id(1)",
        "let y: i64? = None\n    let r: i64 = y ?? id(2)",
        "let y: i64? = None\n    let r: i64 = y ?? id(dbg(2))",
        "let r: Two<u8, i64> = mk(id(5), 1)",
    ] {
        let src = format!(
            "{TRANSPARENT_DECLS}fn main() {{\n    let c = true\n    let x: u8 = 3\n    {row}\n}}"
        );
        clean_row(row, &src);
    }
}

/// A literal does not outrank a concrete argument, whatever form holds it: the concrete one
/// binds the parameter first.
#[test]
fn a_concrete_argument_should_bind_before_a_literal_under_a_transparent_form() {
    // `Result`, `Ok` and `dbg` are the prelude's.
    with_prelude();
    for row in [
        "pick(dbg(1), x)",
        "pick({ 1 }, x)",
        "pick(x, id(1))",
        "pick(1, x)",
    ] {
        let src = format!(
            "{TRANSPARENT_DECLS}fn main() {{\n    let x: u8 = 3\n    let r: u8 = {row}\n}}"
        );
        clean_row(row, &src);
    }
    let src = format!("{TRANSPARENT_DECLS}fn main() {{\n    let r = pick(1, \"s\")\n}}");
    let line = TRANSPARENT_DECLS.lines().count() + 2;
    codes(&src, &[(Code::Mismatch, (line, 18))]);
    // The concrete `"s"` binds first, so the literal under `dbg` is the one reported.
    let src = format!("{TRANSPARENT_DECLS}fn main() {{\n    let r = pick(dbg(1), \"s\")\n}}");
    codes(&src, &[(Code::Mismatch, (line, 18))]);
}

/// A call whose declared return type names none of its own type parameters has a type that its
/// arguments do not give: a method that returns the type parameter of its receiver, or a function
/// that returns a concrete type. It ranks as a call of a callee that is not generic does, so the
/// outer call meets it whole (`u8?`), not its literal arguments.
#[test]
fn a_call_whose_type_its_arguments_do_not_give_should_rank_concrete() {
    with_prelude();
    let decls = "fn id<T>(x: T) -> T { x }\n\
        fn pick<T>(a: T, b: T) -> T { a }\n\
        fn len_of<T>(xs: [T]) -> i64 { 0 }\n\
        class Bx<T> {\n    v: T\n\
        fn get<U>(self, u: U) -> T { self.v }\n\
        fn put<U>(self, x: T, u: U) -> T { x }\n}\n";
    // The requirement call is in `f`, which the other rows do not need.
    let bound = "interface G<A> {\n    fn rget<U>(self, u: U) -> A\n}\n\
        class C {}\n\
        extend C: G<u8?> {\n    fn rget<U>(self, u: U) -> u8? { Some(4) }\n}\n\
        fn f<X: G<u8?>>(x: X) -> u8 {\n    let r: u8? = id(x.rget(1))\n    r ?? 0\n}\n";
    for (row, more) in [
        ("let r: u8? = id(bx.get(1))", ""),
        ("let r: u8? = id(bx.get(Some(1)))", ""),
        ("let r: u8? = id(bx.put(1, 5))", ""),
        ("let r: u8 = f(C())", bound),
        ("let r: u8? = pick(bx.get(1), 2)", ""),
    ] {
        let src = format!(
            "{decls}{more}fn main() {{\n    let v: u8? = Some(3)\n    let bx = Bx(v: v)\n    {row}\n}}"
        );
        clean_row(row, &src);
    }
    // The call is `i64` whatever its argument is, so it binds before `x`, which is then the one
    // reported.
    let src =
        format!("{decls}fn main() {{\n    let x: u8 = 3\n    let r = pick(len_of([1]), x)\n}}");
    let line = decls.lines().count() + 3;
    codes(&src, &[(Code::Mismatch, (line, 31))]);
}

/// A call of a method, or of a requirement of a bound, that returns `U?` makes an optional by
/// itself, as `Some(...)` and a function that returns `T?` do. The call that holds it meets an
/// optional context whole (`u8?`). An argument of the call that holds it can be at a later place
/// than the first.
#[test]
fn a_method_that_returns_an_optional_should_make_the_call_that_holds_it_optional() {
    with_prelude();
    let decls = "fn id<T>(x: T) -> T { x }\n\
        fn pick<T>(a: T, b: T) -> T { a }\n\
        fn pick3<T>(a: T, b: T, c: T) -> T { a }\n\
        class Bx<T> {\n    v: T\n    fn mk<U>(self, u: U) -> U? { Some(u) }\n}\n\
        interface G {\n    fn ropt<U>(self, u: U) -> U?\n}\n\
        class C {}\n\
        extend C: G {\n    fn ropt<U>(self, u: U) -> U? { Some(u) }\n}\n\
        fn f<X: G>(x: X) -> u8? { id(x.ropt(1)) }\n";
    for row in [
        // A method of the receiver's class.
        "let r: u8? = id(bx.mk(1))",
        // A requirement of a bound, in the body of `f` above.
        "let r: u8? = f(C())",
        // The optional argument is at the second and the third place.
        "let r: u8? = pick(1, Some(2))",
        "let r: u8? = pick3(1, 2, Some(3))",
    ] {
        let src = format!(
            "{decls}fn main() {{\n    let bx = Bx(v: 1)\n    {row}\n    print(\"{{r ?? 0}}\")\n}}"
        );
        clean_row(row, &src);
    }
}

/// With no context, the arguments that hold an optional are checked before a plain literal beside
/// them, so the parameter is the optional type and the literal fits it. Before, the literal bound
/// the parameter to `i64` first and the optional was rejected. A value holds an optional inside an
/// array, and in a call that has such an argument at a place whose type names a type parameter
/// that the return type names too: `wrap(Some(2))`, `unbox(Bx(v: Some(2)))`. The order of the
/// arguments, and a context of `i64?` or `u8?`, make no difference. A `Some` that the callee does
/// not return (`k2(Some(1), [2])`) does not make its call hold an optional.
#[test]
fn an_optional_argument_should_bind_before_a_plain_literal() {
    with_prelude();
    let decls = "fn pick<T>(a: T, b: T) -> T { a }\n\
        fn pick3<T>(a: T, b: T, c: T) -> T { a }\n\
        fn opt<T>(x: T) -> T? { Some(x) }\n\
        fn id<T>(x: T) -> T { x }\n\
        fn k<T>(a: T, b: [T]) -> T { a }\n\
        fn first<T>(xs: [T]) -> T { xs[0] }\n\
        fn k2<T, U>(a: T, b: U) -> U { b }\n\
        fn h<T>(a: T, b: T?) -> T { a }\n\
        fn wrap<T>(x: T) -> [T] { [x] }\n\
        fn g<T>(xs: [T]) -> [T] { xs }\n\
        fn two<T>(a: T, b: i64) -> [T] { [a] }\n\
        fn mk<T>(x: T) -> Bx<T> { Bx(v: x) }\n\
        fn unbox<T>(b: Bx<T>) -> T { b.v }\n\
        class Bx<T> {\n    v: T\n}\n\
        class Holder {\n    fn pick<T>(self, a: T, b: T) -> T { a }\n}\n";
    // The reversed orders passed before this change: they are controls.
    for row in [
        "let v = pick(1, Some(2))",
        "let v = pick(1, opt(2))",
        "let v = pick(1, if c { 2 } else { None })",
        "let v = pick(1, match c {\n true => Some(2)\n false => None\n })",
        "let v = pick(1, dbg(Some(2)))",
        "let v = pick(1, { Some(2) })",
        "let v = pick(Some(2), 1)",
        "let v = pick(opt(2), 1)",
        "let v = pick(if c { 2 } else { None }, 1)",
        "let v = pick3(1, 2, Some(3))",
        "let v = pick3(1, Some(2), 3)",
        "let v = pick3(Some(1), 2, 3)",
        "let v = h(1, Some(2))",
        "let v = holder.pick(1, Some(2))",
        "let v: i64? = pick(1, Some(2))",
        "let v: u8? = pick(1, Some(2))",
        "let v: u8? = pick3(1, 2, Some(3))",
        "let v: u8? = holder.pick(1, opt(2))",
        // The optional is inside an array literal, or in a value that passes one on.
        "let v = pick([1], [Some(2)])",
        "let v = pick([1], [None, Some(2)])",
        "let v = pick([[1]], [[Some(2)]])",
        "let v = pick([1], id([Some(2)]))",
        "let v = k(1, [Some(2)])",
        "let v = pick([Some(2)], [1])",
        "let v = pick([[Some(2)]], [[1]])",
        "let v: [i64?] = pick([1], [Some(2)])",
        "let v: [u8?] = pick([1], [Some(2)])",
        // A call that wraps the optional in an array, a class or an enum.
        "let v = pick([1], wrap(Some(2)))",
        "let v = pick([1], wrap(opt(2)))",
        "let v = pick([1], wrap(if c { 2 } else { None }))",
        "let v = pick([[1]], wrap([Some(2)]))",
        "let v = pick([1], g([Some(2)]))",
        "let v = pick([1], two(Some(2), 1))",
        "let v = pick(Bx(v: 1), mk(Some(2)))",
        "let v = pick(1, unbox(Bx(v: Some(2))))",
        "let v = pick(1, first(wrap(Some(2))))",
        "let v = k(1, wrap(Some(2)))",
        "let v: i64? = pick(1, unbox(Bx(v: Some(2))))",
        "let v: u8? = k(1, wrap(Some(2)))",
        // The reversed orders passed before this change: they are controls.
        "let v = pick(wrap(Some(2)), [1])",
        "let v = pick(g([Some(2)]), [1])",
        "let v = pick(unbox(Bx(v: Some(2))), 1)",
        "let v = pick(mk(Some(2)), Bx(v: 1))",
        // The `Some` is not returned, so the call holds no optional: the array binds first.
        "let v = pick([1], k2(Some(1), [2]))",
        "let v = pick(k2(Some(1), [2]), [Some(3)])",
    ] {
        let src = format!(
            "{decls}fn main() {{\n    let c = true\n    let holder = Holder()\n    {row}\n}}"
        );
        clean_row(row, &src);
    }
}

/// A context of `i64?` or `u8?` meets the return type whole, and not its payload, when an argument
/// holds the optional at a place of the returned parameter, with no plain literal beside it. The
/// place can be in an array or a class, also in a call that is the argument. Before, the context
/// met the payload, and `Some(1)` was rejected.
#[test]
fn a_context_should_meet_a_call_whose_argument_holds_an_optional() {
    with_prelude();
    let decls = "fn first<T>(xs: [T]) -> T { xs[0] }\n\
        fn k<T>(a: T, b: [T]) -> T { a }\n\
        fn wrap<T>(x: T) -> [T] { [x] }\n\
        fn unbox<T>(b: Bx<T>) -> T { b.v }\n\
        class Bx<T> {\n    v: T\n}\n";
    for row in [
        "let v: i64? = k(1, [Some(2)])",
        "let v: i64? = first([Some(1)])",
        "let v: u8? = first([Some(1)])",
        "let v: i64? = unbox(Bx(v: Some(1)))",
        "let v: u8? = unbox(Bx(v: Some(1)))",
        "let v: i64? = first(wrap(Some(1)))",
        "let v: u8? = first(wrap(Some(1)))",
        "let v: i64? = k(1, wrap(Some(2)))",
    ] {
        let src = format!("{decls}fn main() {{\n    {row}\n}}");
        clean_row(row, &src);
    }
}

/// A safe call (`receiver?.method(...)`) has an optional result, whatever the method returns, so
/// the call that holds it meets an optional context whole (`i64?`). Before, only a method that
/// returns an optional counted, and `id(b?.m(1))` reported "expected `i64`, found `i64?`".
#[test]
fn a_safe_call_should_make_the_call_that_holds_it_optional() {
    with_prelude();
    let decls = "fn id<T>(x: T) -> T { x }\n\
        fn pick<T>(a: T, b: T) -> T { a }\n\
        class B {\n    fn m<T>(self, x: T) -> T { x }\n}\n\
        interface R {\n    fn r<T>(self, x: T) -> T\n}\n\
        class Q: R {\n    fn r<T>(self, x: T) -> T { x }\n}\n\
        fn via<X: R>(x: X?) -> i64? { id(x?.r(1)) }\n";
    for row in [
        "let r: i64? = id(b?.m(1))",
        "let r: u8? = id(b?.m(1))",
        "let r = id(b?.m(1))",
        "let r: i64? = id(dbg(b?.m(1)))",
        "let r: i64? = id({ b?.m(1) })",
        "let r: i64? = id(if c { b?.m(1) } else { 1 })",
        "let r: i64? = pick(1, b?.m(1))",
        "let r: i64? = pick(b?.m(1), 1)",
        "let r: i64? = pick(None, b?.m(1))",
        "let r = pick(1, b?.m(1))",
        "let r = [b?.m(1), 2]",
        "let r = b?.m(1) == 2",
        "let r = via(q)",
    ] {
        let src = format!(
            "{decls}fn main() {{\n    let c = true\n    let b: B? = B()\n    let q: Q? = Q()\n    {row}\n}}"
        );
        clean_row(row, &src);
    }
}

/// A name that a branch binds hides an outer local of the same name while ranking, as it does
/// while checking: the call on the branch's local is the one that runs, and the outer local has
/// no method `m` to be checked against it.
#[test]
fn a_branch_local_should_hide_an_outer_local_while_ranking() {
    with_prelude();
    for row in [
        "let r: u8? = id(if c {\n let b = B()\n none_of(b.m(5))\n } else {\n Some(1)\n })",
        "let r: u8? = id(match o {\n Some(b) => none_of(b.m(5))\n None => Some(1)\n })",
        "let r: u8? = id(if let b = o {\n none_of(b.m(5))\n } else {\n Some(1)\n })",
    ] {
        let src = format!(
            "{TRANSPARENT_DECLS}class A {{\n    x: i64\n}}\n\
             class B {{\n    fn m<T>(self, x: T) -> T {{ x }}\n}}\n\
             fn main() {{\n    let c = true\n    let o: B? = B()\n    let b = A(x: 1)\n    {row}\n    print(\"{{b.x}}\")\n}}"
        );
        clean_row(row, &src);
    }
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
        // The expected type fits the result alone, and `n` contradicts it.
        (
            "fn g() {\n    let n: i64 = 1\n    let x: f64 = id(n)\n}",
            vec![(Code::Mismatch, (3, 21))],
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
        // Only `Option` has the variants `Some` and `None`.
        (
            "fn g() {\n    let x = Tree.Some<i64>(1)\n}",
            vec![(Code::UnknownVariant, (2, 18))],
        ),
        (
            "fn g(c: Cell<i64>?) -> bool {\n    Tree<i64>.None is c\n}",
            vec![(Code::UnknownVariant, (2, 15))],
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

/// An argument that binds a type parameter apart from the call's context is reported at the
/// argument, in every kind of generic call, and the call adds no error of its own.
#[test]
fn an_argument_that_contradicts_the_context_should_be_reported_at_the_argument() {
    with_prelude();
    for (line, at) in [
        // A built-in variant, a function, a user variant, a class and a method.
        (
            "fn g() {\n    let r: Result<i64, string> = Ok(\"x\")\n}",
            (2, 37, "\"x\""),
        ),
        (
            "fn g() {\n    let r: Result<i64, string> = Ok(1.5)\n}",
            (2, 37, "1.5"),
        ),
        ("fn g(n: i64) {\n    let b: string = id(n)\n}", (2, 24, "n")),
        (
            "fn g(n: i64) {\n    let o: Opt2<string> = Opt2.S(n)\n}",
            (2, 34, "n"),
        ),
        (
            "fn g(n: i64) {\n    let c: Box<string> = Box(v: n)\n}",
            (2, 33, "n"),
        ),
        (
            "fn g(b: Box<i64>, n: i64) {\n    let r: [string] = b.map_all(n)\n}",
            (2, 33, "n"),
        ),
    ] {
        let src = with_generics(line);
        diagnostic(&src, Code::Mismatch, at, &[]);
    }
}

/// A blamed argument leaves one error for the call: the later arguments meet the type that the
/// context gives, and an argument that was rejected before keeps the call as it is.
#[test]
fn a_blamed_argument_should_be_the_only_error_of_its_call() {
    let pick = "fn pick<T>(a: T, b: T) -> T {\n    a\n}\n";
    for (body, at) in [
        // `1`, `None` and `[]` meet the `T` of the context, not the `string` that `s` bound.
        (
            "fn g(s: string) {\n    let x: [i64] = pair(s, 1)\n}",
            (5, 25, "s"),
        ),
        (
            "fn g(s: string) {\n    let x: [i64?] = pair(s, None)\n}",
            (5, 26, "s"),
        ),
        (
            "fn g(s: string) {\n    let x: [[i64]] = pair(s, [])\n}",
            (5, 27, "s"),
        ),
        // `n` was rejected beside `s` already, which is the only error.
        (
            "fn g(s: string, n: i64) {\n    let x: [i64] = pair(s, n)\n}",
            (5, 28, "n"),
        ),
        (
            "fn g(s: string, n: i64) {\n    let x: string = pick(n, s)\n}",
            (5, 29, "s"),
        ),
    ] {
        let src = with_generics(&format!("{pick}{body}"));
        diagnostic(&src, Code::Mismatch, at, &[]);
    }
    // The context leaves `B` open, so `s` keeps it, and the literal `1` is checked against it.
    let src = with_generics(
        "fn two_b<A, B>(a: A, b: B, c: B) -> [A] {\n    [a]\n}\nfn g(n: i64, s: string) {\n    let x: [string] = two_b(n, s, 1)\n}",
    );
    codes(
        &src,
        &[(Code::Mismatch, (5, 29)), (Code::Mismatch, (5, 35))],
    );
    // `panic` never finishes, so it has no value, and no type to blame.
    let src = with_generics(&format!(
        "{pick}fn g(n: i64) {{\n    let x: [string] = pair(panic(\"x\"), n)\n}}"
    ));
    codes(&src, &[(Code::NoValue, (5, 28)), (Code::Mismatch, (5, 40))]);
    // A `unit` argument has no value, which its own error says.
    let src = with_generics(&format!(
        "{pick}fn u() {{}}\nfn g(n: i64) {{\n    let x: [string] = pair(u(), n)\n}}"
    ));
    codes(&src, &[(Code::NoValue, (6, 28)), (Code::Mismatch, (6, 33))]);
}

/// Type arguments that the call writes are the call's own, so a mismatch with the context stays
/// at the call.
#[test]
fn a_call_with_written_type_arguments_should_report_a_mismatch_at_the_call() {
    for (body, at) in [
        (
            "fn g(n: i64) {\n    let x: f64 = id<i64>(n)\n}",
            (2, 18, "id<i64>(n)"),
        ),
        (
            "fn g(n: i64) {\n    let x: string = id<i64>(n)\n}",
            (2, 21, "id<i64>(n)"),
        ),
        (
            "fn g(n: i64) {\n    let x: Box<f64> = Box<i64>(v: n)\n}",
            (2, 23, "Box<i64>(v: n)"),
        ),
    ] {
        let src = with_generics(body);
        diagnostic(&src, Code::Mismatch, at, &[]);
    }
}

/// Only an argument that contradicts the context is reported: one that fits, or that no
/// parameter of the context reaches, is not. The context meets the type of the call whole, or
/// by its payload.
#[test]
fn an_argument_that_fits_the_context_should_not_be_reported() {
    let two = "class Two<A, B> {\n    a: A\n    b: B\n}\nfn both<A, B>(a: A, b: B) -> Two<A, B> {\n    Two(a: a, b: b)\n}\nfn first_of<A, B>(a: A, b: B) -> [A] {\n    [a]\n}\n";
    for (body, at) in [
        // `s` fits `string`, which the context gives `B`.
        (
            "fn g(n: i64, s: string) {\n    let x: Two<f64, string> = both(n, s)\n}",
            (12, 36, "n"),
        ),
        // The context gives `B` nothing, and `s` binds it.
        (
            "fn g(n: i64, s: string) {\n    let x: [string] = first_of(n, s)\n}",
            (12, 32, "n"),
        ),
        // The type of the call meets the payload of an optional context.
        (
            "fn g(s: string) {\n    let x: [i64]? = wrap(s)\n}",
            (12, 26, "s"),
        ),
        // The context is an optional that the argument's type is not.
        (
            "fn g(s: string) {\n    let x: i64? = id(s)\n}",
            (12, 22, "s"),
        ),
    ] {
        let src = with_generics(&format!("{two}{body}"));
        diagnostic(&src, Code::Mismatch, at, &[]);
    }
}

/// A context that the call's type can meet, whole or by its payload, leaves a call clean, and one
/// it cannot meet by itself keeps the error at the call.
#[test]
fn a_context_that_fits_or_does_not_fit_alone_should_keep_its_report() {
    clean(&with_generics(
        "fn g(z: i64?) {\n    let y: i64?? = id(z)\n}",
    ));
    clean(&with_generics("fn g() {\n    let x: i64? = id(5)\n}"));
    clean(&with_generics("fn g(n: i64) {\n    let x: i64? = id(n)\n}"));
    // `[T]` meets no `string`, so the context says nothing of the argument.
    let src = with_generics("fn g(n: i64) {\n    let s: string = wrap(n)\n}");
    diagnostic(&src, Code::Mismatch, (2, 21, "wrap(n)"), &[]);
    // `T?` meets a value that is not optional, which a result cannot: `string?` is no `string`.
    let src = with_generics("fn g(ns: [i64]) {\n    let x: string = first(ns)\n}");
    diagnostic(&src, Code::Mismatch, (2, 21, "first(ns)"), &[]);
    let src = with_generics("fn g(ns: [string]) {\n    let x: string = first(ns)\n}");
    diagnostic(&src, Code::Mismatch, (2, 21, "first(ns)"), &[]);
    // `n` converts to the `i64?` that the context makes of `T`, so no argument is to blame.
    let src = with_generics("fn g(n: i64) {\n    let xs: [i64?] = wrap(n)\n}");
    diagnostic(&src, Code::Mismatch, (2, 22, "wrap(n)"), &[]);
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
        // The expected type fits the result alone, and `n` for `A` is reported against it.
        (
            format!(
                "{two}fn both<A, B>(a: A, b: B) -> Two<A, B> {{ Two(a: a, b: b) }}\nfn g(n: i64) {{\n    let x: Two<f64, [i64]> = both(n, [])\n}}"
            ),
            vec![(Code::Mismatch, (7, 35))],
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
        // The `[1]` after it binds `T`, so the `[None]` before it is no mistake (#300).
        ("g4([[None], [1]])", vec![]),
        // Nothing binds `T`: the call reports it, once.
        ("g4([[None], [None]])", vec![(Code::CannotInfer, (2, 5))]),
    ] {
        codes(&with_optional_elements(body), &expected);
    }
}

/// `f` takes a parameter that its argument only partly fixes: `string` is fixed, `T` is not.
const PARTLY_BOUND: &str = "fn f<T>(r: Result<T, string>) -> T { panic(\"e\") }\n";

/// An argument that waits for context, and is itself a generic call, binds the rest of a
/// partly bound parameter: `Ok(1)` takes `string` from `f`'s parameter, and `f` takes `T` from
/// `Ok(1)`.
#[test]
fn waiting_generic_call_should_bind_the_rest_of_a_partly_bound_parameter() {
    with_prelude();
    let src = format!("{PARTLY_BOUND}fn main() {{\n    let x = f(Ok(1))\n    print(x)\n}}");
    assert_eq!(call_types(&src, "f"), ["i64", "i64"]);
}

/// A call of `f` with a method call as the waiting argument, for a callee that is not a
/// variant: the argument is a generic call of any kind.
#[test]
fn waiting_generic_method_call_should_bind_the_rest_of_a_partly_bound_parameter() {
    with_prelude();
    let src = "fn f<T>(r: Result<T, string>) -> T { panic(\"e\") }\n\
               class Maker {\n    fn make<U, E>(self, u: U) -> Result<U, E> { Ok(u) }\n}\n\
               fn main() {\n    let m = Maker()\n    let x = f(m.make(1))\n    print(x)\n}";
    assert_eq!(call_types(src, "f"), ["i64", "i64"]);
}

/// `T` sits below the top of the parameter type, so the context for `Ok` is `Result<[_], string>`
/// and its `T` meets `[_]`, a type with a hole: the hole binds nothing.
#[test]
fn hole_below_the_top_of_a_parameter_should_bind_nothing() {
    with_prelude();
    let src = "fn f<T>(r: Result<[T], string>) -> [T] { panic(\"e\") }\n\
               fn main() {\n    let x = f(Ok([1]))\n    print(x.len())\n}";
    assert_eq!(call_types(src, "f"), ["i64", "[i64]"]);
}

/// A waiting `if` keeps the rule it had: it waits, and `f` cannot infer `T`.
#[test]
fn waiting_branch_should_keep_waiting_when_a_parameter_is_partly_bound() {
    with_prelude();
    let src = format!(
        "{PARTLY_BOUND}fn main() {{\n    let c = true\n    let x = f(if c {{ Ok(1) }} else {{ Ok(2) }})\n    print(x)\n}}"
    );
    codes(&src, &[(Code::CannotInfer, (4, 13))]);
}

/// The context for an inner call has a hole where the outer call binds nothing yet. A part of
/// the inner call's return type that names no parameter (`i64`) meets that hole and binds
/// nothing from it.
#[test]
fn context_hole_should_meet_a_return_type_part_that_names_no_parameter() {
    with_prelude();
    let src = format!(
        "{PARTLY_BOUND}fn mk<E>() -> Result<i64, E> {{ panic(\"e\") }}\n\
         fn main() {{\n    let x = f(mk())\n    print(x)\n}}"
    );
    assert_eq!(call_types(&src, "f"), ["i64", "i64"]);
    assert_eq!(call_types(&src, "mk"), ["string", "Result<i64, string>"]);
}

/// `Result<T?, string>` gives `Ok` the context `Result<_?, string>`: its `T` meets `_?`, a part
/// that holds a hole, and binds nothing.
#[test]
fn parameter_of_a_return_type_should_meet_a_part_that_holds_a_hole() {
    with_prelude();
    let src = "fn g<T>(r: Result<T?, string>) -> T { panic(\"e\") }\n\
               fn main() {\n    let x = g(Ok(Some(1)))\n    print(x)\n}";
    assert_eq!(call_types(src, "g"), ["i64", "i64"]);
}

/// A call whose return type has a structure of its own where the context has a hole (`[U]`
/// against `_`) binds nothing from it: `U` is left for the call to report.
#[test]
fn context_hole_should_leave_a_parameter_below_it_unbound() {
    with_prelude();
    let src = format!(
        "{PARTLY_BOUND}fn mk_all<U, E>() -> Result<[U], E> {{ panic(\"e\") }}\n\
         fn main() {{\n    let x = f(mk_all())\n    print(x)\n}}"
    );
    codes(&src, &[(Code::CannotInfer, (4, 15))]);
}

/// A parameter type of `[T]` gives `Ok` an array context that it cannot meet: the error is
/// E0032 at `Ok(1)`, the whole argument, and `Ok` reports no `E`.
#[test]
fn context_that_no_variant_meets_should_be_reported_as_a_mismatch_at_the_argument() {
    with_prelude();
    let src = "fn take<T>(xs: [T]) -> T { panic(\"e\") }\n\
               fn main() {\n    let x = take(Ok(1))\n    print(x)\n}";
    codes(src, &[(Code::Mismatch, (3, 18))]);
}

/// A call whose return type conflicts with a bound part of the context (`string` against
/// `bool`) is reported at the call as a mismatch, and `mk` reports no `E`.
#[test]
fn waiting_call_that_conflicts_in_a_bound_part_should_mismatch_at_the_call() {
    with_prelude();
    let src = "class Two<A, B> {\n    a: A\n    b: B\n}\n\
               fn f<T>(p: Two<T, bool>) {}\n\
               fn mk<E>() -> Two<E, string> { panic(\"e\") }\n\
               fn main() {\n    f(mk())\n}";
    codes(src, &[(Code::Mismatch, (8, 7))]);
}

/// A part of the return type that names no parameter, but meets a part of the context that
/// holds a hole, must still meet its structure: `Result<i64, i64>` against `Result<T, string>`
/// and `Result<i64, bool>` against `Result<_, string>` clash, so each call is a mismatch and
/// `mk` reports no `E`.
#[test]
fn no_parameter_part_should_not_meet_a_value_that_holds_a_hole() {
    with_prelude();
    let src = "class Two<A, B> {\n    a: A\n    b: B\n}\n\
               fn f<T, U>(p: Two<Result<T, string>, U>) -> T { panic(\"e\") }\n\
               fn mk<E>() -> Two<Result<i64, i64>, [E]> { panic(\"e\") }\n\
               fn main() {\n    let x = f(mk())\n    print(x)\n}";
    codes(src, &[(Code::Mismatch, (8, 15))]);
    let src = "fn f<T>(r: Result<T, string>) -> T { panic(\"e\") }\n\
               fn mk<E>(x: E?) -> Result<i64, bool> { panic(\"e\") }\n\
               fn main() {\n    let x = f(mk(None))\n    print(x)\n}";
    codes(src, &[(Code::Mismatch, (4, 15))]);
}

/// A call whose own return type is optional cannot meet a context that is not optional, where
/// it holds a hole. The call reports E0032 at itself, as a safe call does, and `mk` reports no
/// `E`.
#[test]
fn optional_result_should_mismatch_a_context_that_is_not_optional() {
    with_prelude();
    let src = "class Two<A, B> {\n    a: A\n    b: B\n}\n\
               fn f<T>(r: Two<T, string>) {}\n\
               fn mk<E>() -> Two<E, string>? { panic(\"e\") }\n\
               fn main() {\n    f(mk())\n}";
    codes(src, &[(Code::Mismatch, (8, 7))]);
}

/// A safe call has an optional result, so a context that is not optional cannot meet it: one
/// E0032 at the call, and the method reports no `E`.
#[test]
fn safe_call_should_mismatch_a_context_that_is_not_optional() {
    with_prelude();
    let src = "class Two<A, B> {\n    a: A\n    b: B\n}\n\
               class Maker {\n    n: i64\n    fn make<E>(self) -> Two<E, string> { panic(\"e\") }\n}\n\
               fn f<T>(r: Two<T, string>) {}\n\
               fn main() {\n    let m: Maker? = None\n    f(m?.make())\n}";
    codes(src, &[(Code::Mismatch, (12, 7))]);
}

/// A waiting call with no value is E0043, as a call with its type written is. It is not a
/// mismatch of `unit`, and a safe call has no `unit?`.
#[test]
fn waiting_call_with_no_value_should_report_no_value() {
    with_prelude();
    let maker = "class Maker {\n    n: i64\n    fn make<E>(self, x: E?) {}\n}\n";
    for (decls, lets, call) in [
        ("fn mk<E>(x: E?) {}\n", "", "f(mk(None))"),
        ("fn mk<E>(x: E?) {}\n", "", "f(mk<i64>(None))"),
        (maker, "let m: Maker? = None\n    ", "f(m?.make(None))"),
        (maker, "let m: Maker? = None\n    ", "f(m?.make<i64>(None))"),
    ] {
        let src = format!("{decls}fn f<T>(r: [T]) {{}}\nfn main() {{\n    {lets}{call}\n}}");
        let line = decls.lines().count() + 3 + lets.matches('\n').count();
        codes(&src, &[(Code::NoValue, (line, 7))]);
    }
}

/// `mk` has a mistake of its own, an argument of the wrong type, and a result that cannot meet
/// the context. It reports the argument only: the mismatch of the result would be a second error
/// for one mistake.
#[test]
fn waiting_call_with_its_own_mistake_should_report_only_that_mistake() {
    with_prelude();
    let src = "class Two<A, B> {\n    a: A\n    b: B\n}\n\
               fn f<T>(p: Two<T, bool>) {}\n\
               fn mk<E>(n: i64) -> Two<E, string> { panic(\"e\") }\n\
               fn main() {\n    f(mk(\"x\"))\n}";
    codes(src, &[(Code::Mismatch, (8, 10))]);
}

/// `Ok(Ok(1))` for `[T]`: the outer `Ok` waits for its argument, which gets no context, so the
/// outer `Ok` reports the mismatch at itself, and the inner `Ok` reports nothing.
#[test]
fn nested_waiting_call_that_cannot_fit_should_mismatch_once_at_the_outer_call() {
    with_prelude();
    let src = "fn take<T>(xs: [T]) -> T { panic(\"e\") }\n\
               fn main() {\n    let x = take(Ok(Ok(1)))\n    print(x)\n}";
    codes(src, &[(Code::Mismatch, (3, 18))]);
}

/// `Some(...)` checks its argument against the payload of its expected type with no binding,
/// so a hole in a context would reach its diagnostics and its type. When the argument is `None`
/// or `[]`, the context fixes nothing: the call waits, as before, and `f` reports its `T`. A
/// leak would give a mismatch or a panic.
#[test]
fn some_of_none_or_an_empty_array_should_leave_the_parameter_uninferred() {
    with_prelude();
    for (decls, call) in [
        ("fn f<T>(x: T?) -> T { panic(\"e\") }\n", "f(Some(None))"),
        (
            "fn f<T>(x: T?) -> T { panic(\"e\") }\n",
            "f(Option.Some(None))",
        ),
        ("fn f<T>(x: [T]?) -> T { panic(\"e\") }\n", "f(Some([]))"),
    ] {
        let src = format!("{decls}fn main() {{\n    let y = {call}\n    print(y)\n}}");
        let line = decls.lines().count() + 2;
        codes(&src, &[(Code::CannotInfer, (line, 13))]);
    }
}

/// When the argument of `Some(...)` is a call, the call binds its own parameter from the
/// context. So `Some(Ok(1))` binds `T` of `f` as `i64`.
#[test]
fn some_of_a_call_should_bind_from_a_partly_bound_parameter() {
    with_prelude();
    let src = "fn f<T>(x: Result<T, string>?) -> T { panic(\"e\") }\n\
               fn main() {\n    let y = f(Some(Ok(1)))\n    print(y)\n}";
    assert_eq!(call_types(src, "f"), ["i64", "i64"]);
}

/// `mk()` gets a context and binds `T`, which a `Some([])` before it waited for: the sweep
/// that follows checks that `Some([])` against `[i64]?`, so the call has no error.
#[test]
fn some_should_be_checked_once_a_later_waiting_call_binds_its_parameter() {
    with_prelude();
    let src = "fn g<T>(x: [T]?, r: Result<T, string>) -> T { panic(\"e\") }\n\
               fn mk<E>() -> Result<i64, E> { panic(\"e\") }\n\
               fn main() {\n    let y = g(Some([]), mk())\n    print(y)\n}";
    assert_eq!(call_types(src, "g"), ["i64", "i64"]);
}

/// Each row writes `T.Item` in the first parameter, through an array, an optional, a class, an
/// enum or not at all, and gives it a generic call. The second argument is a call that waits for
/// the `U` that the third gives, and then binds `T`.
#[test]
fn an_associated_type_argument_should_wait_for_the_argument_that_binds_its_base() {
    const DECLS: &str = "interface I {\n    type Item\n}\n\
        enum Wrap<T> {\n    N\n    X(T)\n}\n\
        class Box<T> {\n    value: T\n}\n\
        class W<T>: I {\n    type Item = Wrap<T>\n}\n\
        enum Two<A, B> {\n    P(A, B)\n}\n\
        fn pair<E>(x: W<i64>) -> Two<W<i64>, E?> { Two.P(x, None) }\n\
        fn id<X>(x: X) -> X { x }\n";
    for (param, arg) in [
        ("T.Item", "id(item)"),
        ("[T.Item]", "id([item])"),
        ("T.Item?", "id(some)"),
        ("Box<T.Item>", "Box(value: item)"),
        ("Wrap<T.Item>", "Wrap.X(item)"),
    ] {
        let src = format!(
            "{DECLS}fn f<T: I, U>(_a: {param}, _b: Two<T, U>, _c: U) {{}}\n\
             fn main() {{\n    let w: W<i64> = W()\n    let item: Wrap<i64> = Wrap.N\n    \
             let some: Wrap<i64>? = None\n    let n: i64? = 1\n    f({arg}, pair(w), n)\n}}"
        );
        clean_row(param, &src);
    }
}

/// A variant built from integer literals ranks as a literal does, so it takes its type from a
/// concrete argument beside it: the mismatch is at the `5`, as for a bare `5`, and not a failed
/// bound of the `i64` that the pair would bind alone.
#[test]
fn a_variant_of_literals_should_take_its_type_from_a_concrete_argument() {
    for (decls, call, col) in [
        (
            "fn h<T: I>(_p: Two<T, T.Item>, _a: T) {}\n",
            "h(Two.P(5, 7), W())",
            13,
        ),
        ("fn h<T: I>(_a: T, _b: T) {}\n", "h(5, W())", 7),
    ] {
        let (src, line) = assoc_program(decls, call);
        codes_row(
            &format!("{decls}{call}"),
            &src,
            &[(Code::Mismatch, (line, col))],
        );
    }
}

/// The declarations of the tests below: `W` and `P` choose `Item` for the interfaces `I` and
/// `Holds<i64>`.
const ASSOC_DECLS: &str = "interface I {\n    type Item\n}\n\
    interface Holds<Y> {\n    type Item\n}\n\
    enum Wrap<T> {\n    N\n    X(T)\n}\n\
    enum Two<A, B> {\n    P(A, B)\n}\n\
    class W: I {\n    type Item = i64\n}\n\
    class P: Holds<i64> {\n    type Item = string\n}\n\
    fn five() -> i64 { 5 }\n";

/// The `main` of `ASSOC_DECLS` and `decls` that holds `call`, and the line of `call`.
fn assoc_program(decls: &str, call: &str) -> (String, usize) {
    let line = (ASSOC_DECLS.lines().count() + decls.lines().count()) + 2;
    (
        format!("{ASSOC_DECLS}{decls}fn main() {{\n    {call}\n}}"),
        line,
    )
}

/// A parameter type that names a parameter outside an associated type too, in either order, in a
/// nested type, through the bound's interface arguments, in an array or an optional, binds the
/// parameter itself: its argument does not wait, so it is a local, a literal or a call.
#[test]
fn an_argument_should_bind_a_parameter_named_outside_an_associated_type() {
    for (decls, call) in [
        ("fn h<T: I>(_p: Two<T, T.Item>) {}\n", "h(Two.P(W(), 7))"),
        ("fn h<T: I>(_p: Two<T.Item, T>) {}\n", "h(Two.P(7, W()))"),
        (
            "fn h<T: I>(_p: Wrap<Two<T.Item, Wrap<T>>>) {}\n",
            "h(Wrap.X(Two.P(7, Wrap.X(W()))))",
        ),
        (
            "fn h<S: Holds<U>, U>(_s: S, _p: Two<S.Item, U>) {}\n",
            "h(P(), Two.P(\"a\", 4))",
        ),
        (
            "fn h<T: I>(_p: Two<T, T.Item>) {}\n",
            "let p = Two.P(W(), 7)\n    h(p)",
        ),
        (
            "fn h<T: I>(_p: [Two<T, T.Item>]) {}\n",
            "h([Two.P(W(), 7)])",
        ),
        ("fn h<T: I>(_p: Two<T, T.Item>?) {}\n", "h(Two.P(W(), 7))"),
    ] {
        let (src, _) = assoc_program(decls, call);
        clean_row(&format!("{decls}{call}"), &src);
    }
}

/// An argument that disagrees with the type its parameter fixes is reported at the argument,
/// and a parameter that only an associated type names stays uninferred. A parameter bound to a
/// type that does not conform reports its bound alone.
#[test]
fn an_associated_type_argument_should_report_its_own_mistake() {
    /// The declarations, the call, and the codes with their columns on the call's line.
    type Row<'a> = (&'a str, &'a str, &'a [(Code, usize)]);
    let rows: &[Row<'_>] = &[
        (
            "fn h<T: I>(_p: Two<T, T.Item>) {}\n",
            "h(Two.P(W(), \"x\"))",
            &[(Code::Mismatch, 7)],
        ),
        (
            "fn h<T: I>(_p: Two<T.Item, T>) {}\n",
            "h(Two.P(\"x\", W()))",
            &[(Code::Mismatch, 7)],
        ),
        (
            "fn h<S: Holds<U>, U>(_s: S, _p: Two<S.Item, U>) {}\n",
            "h(P(), Two.P(5, 4))",
            &[(Code::Mismatch, 12)],
        ),
        (
            "fn h<S: Holds<U>, U>(_s: S, _w: Wrap<S.Item>) {}\n",
            "h(P(), Wrap.X(\"a\"))",
            &[(Code::CannotInfer, 5)],
        ),
        (
            "fn h<T: I>(_p: Two<T, T.Item>) {}\n",
            "h(Two.P(5, 7))",
            &[(Code::NotConform, 7)],
        ),
        (
            "fn h<T: I>(_p: Two<T.Item, T>) {}\n",
            "h(Two.P(7, 5))",
            &[(Code::NotConform, 7)],
        ),
        // After a failed bound, no later argument is checked against the type that does not
        // exist, in an array of optionals or in a second parameter.
        (
            "fn h<T: I>(_p: [Two<T, T.Item>?]) {}\n",
            "h([None, Two.P(5, 7)])",
            &[(Code::NotConform, 14)],
        ),
        (
            "fn h<T: I>(_p: [Two<T, T.Item>?]) {}\n",
            "h([Two.P(5, 7), Two.P(W(), 8)])",
            &[(Code::NotConform, 8)],
        ),
        (
            "fn h<T: I>(_p: Two<T, T.Item>, _q: Two<T, T.Item>) {}\n",
            // `five()`, not a literal: a literal takes its type from the `W()` that binds `T`.
            "h(Two.P(five(), 7), Two.P(W(), 8))",
            &[(Code::NotConform, 7)],
        ),
        // An argument for a type built from the parameter whose bound failed is not checked.
        (
            "fn h<T: I, U>(_p: Two<T, T.Item>, _a: Two<T, U>, _b: Two<T.Item, U>) {}\n",
            "h(Two.P(five(), 7), Two.P(W(), 1), Two.P(1, 1))",
            &[(Code::NotConform, 7)],
        ),
        // A parameter that is not bound yet when the bound fails is still bound by the next
        // argument, so a later mismatch is reported at it.
        (
            "fn h<T: I, U>(_p: Two<T, T.Item>, _a: U, _b: U) {}\n",
            "h(Two.P(five(), 7), 1, \"s\")",
            &[(Code::NotConform, 7), (Code::Mismatch, 25)],
        ),
        // A pattern that names no parameter with a failed bound still reports its mismatch.
        (
            "fn h<T: I, S: I, U, V>(_p: Two<T, T.Item>, _s: S, _a: U, _b: Two<Two<U, V>, i64>) {}\n",
            "h(Two.P(five(), 7), W(), true, Two.P(Two.P(\"s\", 1), 2))",
            &[(Code::NotConform, 7), (Code::Mismatch, 48)],
        ),
        (
            "fn h<T: I, U>(_p: Two<T, T.Item>, _a: Wrap<U>, _b: Wrap<U>) {}\n",
            "h(Two.P(five(), 7), Wrap.X(five()), Wrap.X(\"s\"))",
            &[(Code::NotConform, 7), (Code::Mismatch, 48)],
        ),
    ];
    for &(decls, call, expected) in rows {
        let (src, line) = assoc_program(decls, call);
        let expected: Vec<_> = expected
            .iter()
            .map(|&(code, col)| (code, (line, col)))
            .collect();
        codes_row(&format!("{decls}{call}"), &src, &expected);
    }
}

/// The declarations of the tests below. `Two`, `Box` and `G` build a type from two parts, by an
/// enum variant, a class and a method; `mk` and `id` are generic calls, and `pr` leaves `F` to
/// its context. `W` chooses `Item` for the interface `I`.
const PARTIAL_DECLS: &str = "enum Two<A, B> {\n    P(A, B)\n}\n\
    class Box<A, B> {\n    a: A\n    b: B\n}\n\
    class Pair<A, B> {\n    a: A\n    b: B\n}\n\
    enum Other<A, B> {\n    Q(A, B)\n}\n\
    class C {\n    fn m<A, B>(self, a: A, b: B) -> Two<A, B> {\n        Two.P(a, b)\n    }\n}\n\
    class G<T> {\n    fn pair<B>(self, a: T, b: B) -> Two<T, B> {\n        Two.P(a, b)\n    }\n}\n\
    fn mk<A, B>(a: A, b: B) -> Two<A, B> { Two.P(a, b) }\n\
    fn okr<E>(x: E) -> Result<E, i64> { Ok(x) }\n\
    fn id<T>(x: T) -> T { x }\n\
    fn pr<E, F>(x: E) -> Two<E, F>? { None }\n\
    interface I {\n    type Item\n}\n\
    class W: I {\n    type Item = i64\n}\n";

/// A row of a table of the partial-context tests. It has the declarations after `PARTIAL_DECLS`
/// and the lines of `main`. It also has the codes, each with the text of the last line where it
/// is reported.
type PartialRow<'a> = (&'a str, &'a str, &'a [(Code, &'a str)]);

/// Checks each row: the program's codes are exactly these, at the first place the text is found in
/// the last line of `main`. A row with no code checks clean.
fn check_partial_rows(rows: &[PartialRow<'_>]) {
    with_prelude();
    for &(decls, body, expected) in rows {
        let src = format!(
            "{PARTIAL_DECLS}{decls}fn main() {{\n    {}\n}}",
            body.replace('\n', "\n    ")
        );
        let line = PARTIAL_DECLS.lines().count() + decls.lines().count() + 1 + body.lines().count();
        let last = body.lines().last().expect("a row should have a call");
        let expected: Vec<_> = expected
            .iter()
            .map(|&(code, at)| {
                let col = 4
                    + last
                        .find(at)
                        .unwrap_or_else(|| panic!("`{at}` in `{last}`"))
                    + 1;
                (code, (line, col))
            })
            .collect();
        codes_row(&format!("{decls}{body}"), &src, &expected);
    }
}

/// The rows of `a_partly_bound_parameter_should_report_the_part_that_is_wrong`.
const PART_ROWS: &[PartialRow<'static>] = {
    use Code::Mismatch;
    const S: &str = "\"s\"";
    const X: &str = "\"x\"";
    &[
        (
            "class D { fn m2<E, F>(self, x: E) -> Two<E, F> { panic(\"e\") } }\nfn h<U, V>(a: U, b: Two<U, V>?) {}\n",
            "let d: D? = D()\nh(true, d?.m2(\"s\"))",
            &[(Mismatch, S)],
        ),
        (
            "class D { fn m2<E, F>(self, x: E) -> Two<E, F> { panic(\"e\") } }\nfn h<U, V>(a: U, b: Two<U, V>?) {}\n",
            "let d: D? = D()\nh(true, id(d?.m2(\"s\")))",
            &[(Mismatch, S)],
        ),
        (
            "class D { fn m2<E, F>(self, x: E) -> Two<E, F> { panic(\"e\") } }\nfn h<U, V>(a: U, b: [Two<U, V>?]) {}\n",
            "let d: D? = D()\nh(true, [d?.m2(\"s\")])",
            &[(Mismatch, S)],
        ),
        (
            "class D { fn m2<E, F>(self, x: E) -> Two<E, F> { panic(\"e\") } }\nfn h<U, V>(a: U, b: Two<U, V>??) {}\n",
            "let d: D? = D()\nh(true, Some(d?.m2(\"s\")))",
            &[(Mismatch, S)],
        ),
        (
            "class D { fn m2<E, F>(self, x: E) -> Two<E, F> { panic(\"e\") } }\nfn hn<U, V>(a: U, b: Two<Two<U, V>?, i64>) {}\n",
            "let d: D? = D()\nhn(true, Two.P(d?.m2(\"s\"), 2))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>) {}\n",
            "h(true, Two.P(\"s\", 1))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Box<U, V>) {}\n",
            "h(true, Box(a: \"s\", b: 1))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>) {}\n",
            "h(true, mk(\"s\", 1))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>?) {}\n",
            "h(true, Two.P(\"s\", 1))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>]) {}\n",
            "h(true, [Two.P(\"s\", 1)])",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>]) {}\n",
            "h(true, [Two.P(true, 1), Two.P(\"s\", 2)])",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>?) {}\n",
            "h(true, Some(Two.P(\"s\", 1)))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<Two<U, V>, i64>) {}\n",
            "h(true, Two.P(Two.P(\"s\", 1), 2))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<Two<U, V>, i64>) {}\n",
            "h(true, mk(mk(\"s\", 1), 2))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>?]) {}\n",
            "h(true, [Some(Two.P(\"s\", 1))])",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Result<U, V>) {}\n",
            "h(true, okr(\"s\"))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>?) {}\n",
            "h(true, pr(\"s\"))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>) {}\n",
            "h(true, C().m(\"s\", 1))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>?) {}\n",
            "h(true, Option.Some(Two.P(\"s\", 1)))",
            &[(Mismatch, S)],
        ),
        (
            "fn h2<U, V>(a: U, b: Two<U, V>, c: Two<U, V>) {}\n",
            "h2(true, Two.P(true, 1), Two.P(true, \"x\"))",
            &[(Mismatch, X)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>) {}\n",
            "h(true, id(Two.P(\"s\", 1)))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>?) {}\n",
            "h(true, id(Some(Two.P(\"s\", 1))))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>?) {}\n",
            "h(true, Some(id(Two.P(\"s\", 1))))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: [[Two<U, V>]]) {}\n",
            "h(true, [[Two.P(\"s\", 1)]])",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>?) {}\n",
            "let c: C? = C()\nh(true, c?.m(\"s\", 1))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>??) {}\n",
            "h(true, Some(Some(Two.P(\"s\", 1))))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>??) {}\n",
            "h(true, id(Some(Some(Two.P(\"s\", 1)))))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: [[Two<U, V>?]]) {}\n",
            "h(true, [[None, Some(Two.P(\"s\", 1))]])",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: [[Two<U, V>?]]) {}\n",
            "h(true, [[Some(Two.P(true, 1)), None], [Some(Two.P(false, \"x\"))]])",
            &[(Mismatch, X)],
        ),
        (
            "fn fix<A>(x: A) -> Two<i64, A> { Two.P(1, x) }\nfn h<U, V>(c: V, b: Two<U, V>) {}\n",
            "h(true, fix(\"s\"))",
            &[(Mismatch, S)],
        ),
        (
            "fn fix<A>(x: A) -> [Two<i64, A>] { [] }\nfn h<U, V>(c: V, b: [Two<U, V>]) {}\n",
            "h(true, fix(\"s\"))",
            &[(Mismatch, S)],
        ),
        (
            "fn fix<A>(x: A) -> Two<i64, A>? { None }\nfn h<U, V>(c: V, b: Two<U, V>?) {}\n",
            "h(true, fix(\"s\"))",
            &[(Mismatch, S)],
        ),
        (
            "fn fix<A>(x: A) -> Box<i64, A> { Box(a: 1, b: x) }\nfn h<U, V>(c: V, b: Box<U, V>) {}\n",
            "h(true, fix(\"s\"))",
            &[(Mismatch, S)],
        ),
        (
            "fn mk_array<A, B>(a: A, b: B) -> [Two<A, B>] { [] }\nfn h<U, V>(a: U, b: [Two<U, V>]) {}\n",
            "h(true, mk_array(\"s\", 1))",
            &[(Mismatch, S)],
        ),
        (
            "fn mk_some<A, B>(a: A, b: B) -> Two<A, B>? { None }\nfn h<U, V>(a: U, b: Two<U, V>?) {}\n",
            "h(true, mk_some(\"s\", 1))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, Two<V, i64>>) {}\n",
            "h(true, Two.P(\"s\", Two.P(1, \"x\")))",
            &[(Mismatch, S), (Mismatch, X)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, Two<V, i64>>) {}\n",
            "h(true, Two.P(true, Two.P(1, \"x\")))",
            &[(Mismatch, X)],
        ),
        (
            "fn h<U, V>(a: U, b: Box<U, Box<V, i64>>) {}\n",
            "h(true, Box(a: \"s\", b: Box(a: 1, b: \"x\")))",
            &[(Mismatch, S), (Mismatch, X)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, Two<V, i64>?>) {}\n",
            "h(true, Two.P(\"s\", Some(Two.P(1, \"x\"))))",
            &[(Mismatch, S), (Mismatch, X)],
        ),
        (
            "fn h<T: I, U>(_a: U, _b: [Two<U, T.Item>], _c: T) {}\n",
            "h(true, [Two.P(\"s\", 7)], W())",
            &[(Mismatch, S)],
        ),
        (
            "fn h<T: I, U>(_a: U, _b: Two<U, T.Item>?, _c: T) {}\n",
            "h(true, Some(Two.P(\"s\", 7)), W())",
            &[(Mismatch, S)],
        ),
        (
            "fn h<T: I, U, V>(_a: U, _b: Two<U, Two<V, T.Item>>, _c: T) {}\n",
            "h(true, Two.P(\"s\", Two.P(1, 7)), W())",
            &[(Mismatch, S)],
        ),
        (
            "fn h<T: I, U, V>(_a: Two<T, T.Item>, _b: U, _c: [Two<U, V>]) {}\n",
            "h(Two.P(W(), 7), true, [Two.P(\"s\", 1)])",
            &[(Mismatch, S)],
        ),
        (
            "fn hn<U, V>(a: U, b: Two<Two<U, V>?, i64>) {}\n",
            "let c: C? = C()\nhn(true, Two.P(c?.m(\"s\", 1), 2))",
            &[(Mismatch, S)],
        ),
        (
            "fn h3<U, V>(a: U, b: V, c: Two<U, V>?) {}\n",
            "let c: C? = C()\nh3(true, 1, c?.m(\"s\", None))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>??) {}\n",
            "let c: C? = C()\nh(true, Some(c?.m(\"s\", 1)))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>?]) {}\n",
            "let c: C? = C()\nh(true, [c?.m(\"s\", 1)])",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>?) {}\n",
            "let c: C? = C()\nh(true, id(c?.m(\"s\", 1)))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>) {}\n",
            "h(true, dbg(Two.P(\"s\", 1)))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>) {}\n",
            "h(true, dbg(dbg(Two.P(\"s\", 1))))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>?) {}\n",
            "h(true, Some(dbg(Two.P(\"s\", 1))))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>?) {}\n",
            "h(true, dbg(Some(Two.P(\"s\", 1))))",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>]) {}\n",
            "h(true, [dbg(Two.P(\"s\", 1))])",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>]) {}\n",
            "let t = Two.P(true, 1)\nh(true, [Two.P(\"s\", 1), t])",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>]) {}\n",
            "let t = Two.P(true, 1)\nh(true, [t, Two.P(\"s\", 1)])",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>]) {}\n",
            "let t = Two.P(true, 1)\nh(true, [t, t, Two.P(\"s\", 1), t])",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>]) {}\n",
            "let t = Two.P(true, 1)\nlet ts = [t]\nh(true, [Two.P(\"s\", 1), ts[0]])",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>]) {}\nfn mk0() -> Two<bool, i64> { Two.P(true, 1) }\n",
            "h(true, [mk0(), Two.P(\"s\", 1)])",
            &[(Mismatch, S)],
        ),
        (
            "fn hn<U, V>(a: U, b: [[Two<U, V>]]) {}\n",
            "let t = Two.P(true, 1)\nhn(true, [[t], [Two.P(\"s\", 1), t]])",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>?]) {}\n",
            "let t = Two.P(true, 1)\nh(true, [t, None, Two.P(\"s\", 1)])",
            &[(Mismatch, S)],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>]) {}\n",
            "let t = Two.P(true, 1)\nlet u = Two.P(\"x\", 1)\nh(true, [t, u])",
            &[(Mismatch, "u]")],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>]) {}\n",
            "let t = Two.P(true, 1)\nh(true, [Two.P(true, 1), t, Two.P(\"s\", 2)])",
            &[(Mismatch, S)],
        ),
    ]
};

/// An argument for a parameter that the call binds in part, `b: Two<U, V>` once `a` binds `U`,
/// is checked against what the call binds, with a hole for the rest. So a mistake is reported at
/// the part that is wrong, not at the whole argument against a type that names the callee's `V`.
/// The argument is a call, a construction or a method call. A safe call is one only for an
/// optional parameter. The same holds under `Some`, in an array and in a call that returns its
/// argument. An array element that takes no context, such as a local variable, does not stop
/// the others from taking it.
#[test]
fn a_partly_bound_parameter_should_report_the_part_that_is_wrong() {
    check_partial_rows(PART_ROWS);
}

/// The rows of `a_partly_bound_parameter_should_accept_an_argument_that_fits_it`.
const VALID_ROWS: &[PartialRow<'static>] = {
    &[
        (
            "fn h<U, V>(a: U, b: Two<U, V>) {}\n",
            "let x: u8 = 3\nh(x, Two.P(5, \"x\"))",
            &[],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>]) {}\n",
            "let x: u8 = 3\nh(x, [Two.P(5, \"x\")])",
            &[],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>?) {}\n",
            "let x: u8 = 3\nh(x, Some(Two.P(5, \"x\")))",
            &[],
        ),
        (
            "fn h<U, V>(a: U, b: Two<Two<U, V>, i64>) {}\n",
            "let x: u8 = 3\nh(x, Two.P(Two.P(5, \"x\"), 2))",
            &[],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>) {}\n",
            "h(true, G<bool>().pair(true, 1))",
            &[],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>?) {}\n",
            "let c: C? = C()\nh(true, c?.m(true, 1))",
            &[],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>]?) {}\n",
            "h(true, Some([Two.P(true, 1)]))",
            &[],
        ),
        (
            "fn mk_array<A, B>(a: A, b: B) -> [Two<A, B>] { [] }\nfn h<U, V>(a: U, b: [Two<U, V>]) {}\n",
            "let x: u8 = 3\nh(x, mk_array(5, \"x\"))",
            &[],
        ),
        (
            "fn h<T: I, U, V>(_a: Two<T, T.Item>, _b: U, _c: [Two<U, V>]) {}\n",
            "h(Two.P(W(), 7), true, [Two.P(true, 1)])",
            &[],
        ),
        (
            "fn hn<U, V>(a: U, b: Two<Two<U, V>?, i64>) {}\n",
            "let c: C? = C()\nhn(true, Two.P(c?.m(true, 1), 2))",
            &[],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>?]) {}\nfn mk0() -> Two<bool, i64> { Two.P(true, 1) }\n",
            "let t = Two.P(true, 1)\nh(true, [None, t, Two.P(false, 2), mk0()])",
            &[],
        ),
        (
            "fn hn<U, V>(a: U, b: [[Two<U, V>]]) {}\n",
            "let t = Two.P(true, 1)\nhn(true, [[], [t, Two.P(false, 2)], [t]])",
            &[],
        ),
        // The element type of `mk2` is a bare parameter that no argument binds. The context
        // of the call hints it, so its literal still goes to `inferred_array`.
        (
            "fn mk2<T>(xs: [T]) -> [T] { panic(\"e\") }\nfn h<U, V>(a: U, b: [Two<U, V>]) {}\n",
            "let x: u8 = 3\nh(x, mk2([Two.P(5, \"x\")]))",
            &[],
        ),
    ]
};

/// An argument that fits a parameter bound in part checks. Its literals take the bound type, a
/// `u8` here, as they do for a parameter that the call binds in full.
#[test]
fn a_partly_bound_parameter_should_accept_an_argument_that_fits_it() {
    check_partial_rows(VALID_ROWS);
}

/// The rows of `a_partly_bound_parameter_should_report_the_whole_argument_without_context`.
const WHOLE_ROWS: &[PartialRow<'static>] = {
    use Code::Mismatch;
    &[
        (
            "class D { fn m2<E, F>(self, x: E) -> Two<E, F> { panic(\"e\") } }\nfn h<U, V>(a: U, b: Two<U, V>) {}\n",
            "let d: D? = D()\nh(true, d?.m2(\"s\"))",
            &[(Mismatch, "d?.m2(")],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>) {}\n",
            "h(true, Some(Two.P(\"s\", 1)))",
            &[(Mismatch, "Some(")],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>]) {}\n",
            "h(true, Two.P(\"s\", 1))",
            &[(Mismatch, "Two.P(")],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>) {}\n",
            "h(true, if true { Two.P(\"s\", 1) } else { Two.P(true, 2) })",
            &[(Mismatch, "if"), (Mismatch, "true, 2)")],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>) {}\n",
            "h(true, G<string>().pair(\"s\", 1))",
            &[(Mismatch, "G<")],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>]?) {}\n",
            "h(true, Some([Two.P(\"s\", 1)]))",
            &[(Mismatch, "Some(")],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>]?) {}\n",
            "h(true, [Two.P(\"s\", 1)])",
            &[(Mismatch, "[Two.P")],
        ),
        (
            "fn h<U, V>(a: U, b: Pair<U, Pair<V, i64>>) {}\n",
            "h(true, Box(a: true, b: Pair(a: 1, b: \"x\")))",
            &[(Mismatch, "Box(")],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, Two<V, i64>>) {}\n",
            "h(true, Other.Q(true, Two.P(1, \"x\")))",
            &[(Mismatch, "Other.Q(")],
        ),
        (
            "fn h<U, V>(a: U, b: Two<Pair<U, V>, i64>) {}\n",
            "let x: Box<bool, i64> = Box(a: true, b: 2)\nh(true, G<Box<bool, i64>>().pair(x, \"s\"))",
            &[(Mismatch, "G<Box")],
        ),
        (
            "fn h<U, V>(a: U, b: Two<Other<U, V>, i64>) {}\n",
            "let x: Two<bool, i64> = Two.P(true, 2)\nh(true, G<Two<bool, i64>>().pair(x, \"s\"))",
            &[(Mismatch, "G<Two")],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>) {}\n",
            "let c: C? = C()\nh(true, c?.m(\"s\", 1))",
            &[(Mismatch, "c?.m(")],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>) {}\n",
            "let c: C? = C()\nh(true, c?.m(true, 1))",
            &[(Mismatch, "c?.m(")],
        ),
        (
            "fn hn<U, V>(a: U, b: Two<Two<U, V>, i64>) {}\n",
            "let c: C? = C()\nhn(true, Two.P(c?.m(\"s\", 1), 2))",
            &[(Mismatch, "Two.P(c")],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>?) {}\n",
            "let c: C? = C()\nh(true, Some(c?.m(\"s\", 1)))",
            &[(Mismatch, "Some(")],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>]) {}\n",
            "let c: C? = C()\nh(true, [c?.m(\"s\", 1)])",
            &[(Mismatch, "c?.m(")],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>) {}\n",
            "let c: C? = C()\nh(true, id(c?.m(\"s\", 1)))",
            &[(Mismatch, "id(")],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>]) {}\n",
            "h(true, dbg([Two.P(\"s\", 1)]))",
            &[(Mismatch, "dbg(")],
        ),
    ]
};

/// An argument that gets no context is reported as a whole, as it was before. Its shape may be
/// wrong, as in an optional for a plain parameter. Its kind may get no context: an `if`, an
/// array literal for an optional parameter. The call may return another
/// class or enum than the context. A receiver may fix a part of the type. A safe call, waiting
/// or not, may be for a plain parameter. The reported mistake is then the shape, not a part.
#[test]
fn a_partly_bound_parameter_should_report_the_whole_argument_without_context() {
    check_partial_rows(WHOLE_ROWS);
}

/// The rows of the test with the name
/// `a_partly_bound_parameter_should_report_cannot_infer_when_the_context_fixes_nothing`.
const INFER_ROWS: &[PartialRow<'static>] = {
    use Code::CannotInfer;
    &[
        (
            "fn h<U, V>(a: U, b: Two<U, V>?) {}\n",
            "h(true, pr(true))",
            &[(CannotInfer, "pr(")],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>) {}\n",
            "h(true, Two.P(true, None))",
            &[(CannotInfer, "Two.P(")],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>?) {}\n",
            "h(true, Some(None))",
            &[(CannotInfer, "h(")],
        ),
        (
            "fn h<U, V>(a: U, b: [Two<U, V>]) {}\n",
            "h(true, [])",
            &[(CannotInfer, "h(")],
        ),
        (
            "fn h<U, V>(a: U, b: Two<U, V>??) {}\n",
            "h(true, Some(Some(None)))",
            &[(CannotInfer, "h(")],
        ),
        (
            "fn h3<U, V>(a: U, b: V, c: Two<U, V>?) {}\n",
            "let c: C? = C()\nh3(true, 1, c?.m(true, None))",
            &[(CannotInfer, "m(")],
        ),
    ]
};

/// A context that holds no type for the parameter that is left leaves it to be reported as
/// cannot infer.
#[test]
fn a_partly_bound_parameter_should_report_cannot_infer_when_the_context_fixes_nothing() {
    check_partial_rows(INFER_ROWS);
}

/// The rows of `a_safe_call_that_waits_for_a_plain_context_should_get_no_context`.
const SAFE_ROWS: &[PartialRow<'static>] = {
    use Code::CannotInfer;
    &[
        (
            "class D { fn m2<E, F>(self, x: E) -> Two<E, F> { panic(\"e\") } }\nfn h<U, V>(a: U, b: Two<U, V>) {}\n",
            "let d: D? = D()\nh(true, id(d?.m2(\"s\")))",
            &[(CannotInfer, "id(")],
        ),
        (
            "class D { fn m2<E, F>(self, x: E) -> Two<E, F> { panic(\"e\") } }\nfn h<U, V>(a: U, b: [Two<U, V>]) {}\n",
            "let d: D? = D()\nh(true, [d?.m2(\"s\")])",
            &[(CannotInfer, "h(")],
        ),
        (
            "class D { fn m2<E, F>(self, x: E) -> Two<E, F> { panic(\"e\") } }\nfn hn<U, V>(a: U, b: Two<Two<U, V>, i64>) {}\n",
            "let d: D? = D()\nhn(true, Two.P(d?.m2(\"s\"), 2))",
            &[(CannotInfer, "Two.P(")],
        ),
        (
            "fn h3<U, V>(a: U, b: V, c: Two<U, V>) {}\n",
            "let c: C? = C()\nh3(true, 1, c?.m(\"s\", None))",
            &[(CannotInfer, "m(")],
        ),
    ]
};

/// A safe call has an optional value, so a plain context cannot meet it. A safe call has a hint
/// only as the direct waiting argument of a parameter that is not a bare type parameter. It
/// must also report no mistake of its own. Without the hint, it reports its unbound parameter.
#[test]
fn a_safe_call_that_waits_for_a_plain_context_should_get_no_context() {
    check_partial_rows(SAFE_ROWS);
}

/// The rows of `a_waiting_call_for_a_bare_type_parameter_should_report_its_own_parameter`.
const BARE_ROWS: &[PartialRow<'static>] = {
    use Code::CannotInfer;
    &[
        (
            "fn k2<A>() -> Two<string, A> { panic(\"e\") }\nfn ho<U, V>(a: U, b: Two<U, V>?) {}\n",
            "ho(true, Some(k2()))",
            &[(CannotInfer, "k2(")],
        ),
        (
            "fn k2<A>() -> Two<string, A> { panic(\"e\") }\nfn hn<U, V>(a: U, b: Two<Two<U, V>, i64>) {}\n",
            "hn(true, Two.P(k2(), 2))",
            &[(CannotInfer, "k2(")],
        ),
        (
            "fn k2<A>() -> Two<string, A> { panic(\"e\") }\nfn h<U, V>(a: U, b: Two<U, V>) {}\n",
            "h(true, id(k2()))",
            &[(CannotInfer, "k2(")],
        ),
        (
            "fn k3<A>() -> Two<string, A>? { panic(\"e\") }\nfn ho<U, V>(a: U, b: Two<U, V>?) {}\n",
            "ho(true, id(k3()))",
            &[(CannotInfer, "k3(")],
        ),
        (
            "class K { fn k4<A>(self) -> Two<string, A> { panic(\"e\") } }\nfn ho<U, V>(a: U, b: Two<U, V>?) {}\n",
            "let kk: K? = K()\nho(true, id(kk?.k4()))",
            &[(CannotInfer, "k4(")],
        ),
        (
            "class K { fn k4<A>(self) -> Two<string, A> { panic(\"e\") } }\nfn hno<U, V>(a: U, b: Two<Two<U, V>?, i64>) {}\n",
            "let kk: K? = K()\nhno(true, Two.P(kk?.k4(), 2))",
            &[(CannotInfer, "k4(")],
        ),
    ]
};

/// A call that waits for a bare type parameter, such as `T` of `id` or `A` of `Two.P`, has no
/// hint. A mismatch would name a parameter that the user did not write at the call. So the call
/// reports its own unbound parameter.
#[test]
fn a_waiting_call_for_a_bare_type_parameter_should_report_its_own_parameter() {
    check_partial_rows(BARE_ROWS);
}

/// The rows of `an_expected_type_with_no_hole_should_give_no_hint`.
const NO_HOLE_ROWS: &[PartialRow<'static>] = {
    use Code::Mismatch;
    &[(
        "",
        "let n: i64 = 5\nlet t: Two<u8, i64> = mk(id(n), 1)",
        &[(Mismatch, "id(n)")],
    )]
};

/// The expected type of a call that has no hole is no hint: the arguments of the call get no
/// context from it.
#[test]
fn an_expected_type_with_no_hole_should_give_no_hint() {
    check_partial_rows(NO_HOLE_ROWS);
}

/// The rows of `an_argument_with_labels_out_of_order_should_report_only_the_order`.
const ORDER_ROWS: &[PartialRow<'static>] = {
    use Code::FieldOutOfOrder;
    &[(
        "fn h<U, V>(a: U, b: Box<U, V>) {}\n",
        "h(true, Box(b: 1, a: \"s\"))",
        &[(FieldOutOfOrder, "b: 1")],
    )]
};

/// A construction with its labels out of order reports the order alone.
#[test]
fn an_argument_with_labels_out_of_order_should_report_only_the_order() {
    check_partial_rows(ORDER_ROWS);
}

/// The rows of `a_failed_bound_should_not_hide_the_part_that_is_wrong`.
const BOUND_ROWS: &[PartialRow<'static>] = {
    use Code::{Mismatch, NotConform};
    const S: &str = "\"s\"";
    &[(
        "fn h<T: I, U, V>(_a: Two<T, T.Item>, _b: U, _c: Two<U, V>) {}\n",
        "h(Two.P(5, 7), true, Two.P(\"s\", 1))",
        &[(NotConform, "Two.P(5"), (Mismatch, S)],
    )]
};

/// A failed bound is reported, and a later argument for a parameter bound in part still reports the
/// part that is wrong.
#[test]
fn a_failed_bound_should_not_hide_the_part_that_is_wrong() {
    check_partial_rows(BOUND_ROWS);
}

/// A call that waits inside a call that waits binds the parameter from the inner call's value:
/// `Ok(Ok(1))` for `Result<Result<T, bool>, string>` binds `T = i64`, and `Ok(Ok("s"))` binds
/// `T = string`. The outer call gives the inner one its context, with a hole for `T`.
#[test]
fn a_waiting_call_inside_a_waiting_call_should_bind_the_parameter() {
    with_prelude();
    for (arg, ty) in [("Ok(Ok(1))", "i64"), ("Ok(Ok(\"s\"))", "string")] {
        let src = format!(
            "fn f<T>(r: Result<Result<T, bool>, string>) {{}}\nfn main() {{\n    f({arg})\n}}"
        );
        assert_eq!(call_types(&src, "f"), [ty, "unit"], "{arg}");
    }
}

/// `takes_context` looks at each element of an array literal once. `inferred_array` then gives
/// each element its context with no new walk. So the work for an array literal of generic calls
/// grows with the elements, not with their square. Each element is a call that the context
/// reaches.
#[test]
fn array_literal_context_should_cost_one_step_for_each_element() {
    let steps = |n: usize| {
        let src = format!(
            "enum Two<A, B> {{\n    P(A, B)\n}}\n\
             fn h<U, V>(a: U, b: [Two<U, V>]) {{}}\n\
             fn main() {{\n    h(true, [{}])\n}}",
            vec!["Two.P(true, 1)"; n].join(", ")
        );
        generic::CONTEXT_STEPS.set(0);
        clean(&src);
        generic::CONTEXT_STEPS.get()
    };
    assert!(steps(1) > 0, "the counter should count the walk");
    assert_near_linear(steps(100), steps(200), steps(300));
}

/// An array literal nested `d` deep, for a parameter that is `d` arrays deep. `takes_context`
/// accepts the whole literal once, and `inferred_array` gives each level its element context
/// with no new walk. So the work grows with the depth, not with its square.
#[test]
fn nested_array_literal_context_should_cost_one_step_for_each_level() {
    let steps = |depth: usize| {
        let src = format!(
            "enum Two<A, B> {{\n    P(A, B)\n}}\n\
             fn h<U, V>(a: U, b: {}Two<U, V>{}) {{}}\n\
             fn main() {{\n    h(true, {}Two.P(true, 1){})\n}}",
            "[".repeat(depth),
            "]".repeat(depth),
            "[".repeat(depth),
            "]".repeat(depth),
        );
        generic::CONTEXT_STEPS.set(0);
        clean(&src);
        generic::CONTEXT_STEPS.get()
    };
    assert!(steps(1) > 0, "the counter should count the walk");
    assert_near_linear(steps(10), steps(20), steps(30));
}

/// As the test above, but each level also holds a local variable, which takes no context. So
/// `takes_context` refuses each literal after a walk of the literal inside it, and
/// `inferred_array` asks for the context of each element again. `taken` keeps the answer for
/// each literal, so the work grows with the depth, not with its square.
#[test]
fn nested_array_literal_with_local_variables_should_cost_one_step_for_each_level() {
    let steps = |depth: usize| {
        let body: String = (0..depth)
            .map(|level| match level {
                0 => "    let t0 = Two.P(true, 1)\n".to_string(),
                _ => format!("    let t{level} = [t{}]\n", level - 1),
            })
            .collect();
        let literal = (1..depth).fold("[Two.P(true, 1), t0]".to_string(), |inner, level| {
            format!("[{inner}, t{level}]")
        });
        let src = format!(
            "enum Two<A, B> {{\n    P(A, B)\n}}\n\
             fn h<U, V>(a: U, b: {}Two<U, V>{}) {{}}\n\
             fn main() {{\n{body}    h(true, {literal})\n}}",
            "[".repeat(depth),
            "]".repeat(depth),
        );
        generic::CONTEXT_STEPS.set(0);
        clean(&src);
        generic::CONTEXT_STEPS.get()
    };
    assert!(steps(1) > 0, "the counter should count the walk");
    assert_near_linear(steps(10), steps(20), steps(30));
}

/// A chain of `Some(...)` `d` deep, for an optional parameter that is `d` deep. `takes_context`
/// walks the chain once, and the call of each `Some` asks again with the same context, which
/// `taken` answers. So the work grows with the depth, not with its square.
#[test]
fn nested_some_context_should_cost_one_step_for_each_level() {
    with_prelude();
    let steps = |depth: usize| {
        let src = format!(
            "enum Two<A, B> {{\n    P(A, B)\n}}\n\
             fn h<U, V>(a: U, b: Two<U, V>{}) {{}}\n\
             fn main() {{\n    h(true, {}Two.P(true, 1){})\n}}",
            "?".repeat(depth),
            "Some(".repeat(depth),
            ")".repeat(depth),
        );
        generic::CONTEXT_STEPS.set(0);
        clean(&src);
        generic::CONTEXT_STEPS.get()
    };
    assert!(steps(1) > 0, "the counter should count the walk");
    assert_near_linear(steps(10), steps(20), steps(30));
}

/// A generic call nested `d` deep, as an argument of a generic call: the outer call ranks its
/// arguments, and each inner call ranks its own again when it is checked. A join or an operator
/// nested so ranks its siblings, and each inner one ranks its own again. `body.ranked` answers
/// the second time, so `rank_value` walks each expression once and the work grows with the
/// depth, not with its square. The depths are far below the nesting limit of the parser (256).
#[test]
fn nested_generic_calls_should_rank_each_expression_once() {
    with_prelude();
    // The statement is `before`, then `open` repeated `depth` times, `inner`, `close` likewise.
    let steps = |(before, open, inner, close): (&str, &str, &str, &str), depth: usize| {
        let src = format!(
            "fn k<T>(a: T, b: T) -> T {{ b }}\nfn id<T>(x: T) -> T {{ x }}\n\
             fn main() {{\n    let c = true\n    let x: u8 = 3\n    {before}{}{inner}{}\n}}",
            open.repeat(depth),
            close.repeat(depth),
        );
        generic::RANK_STEPS.set(0);
        clean(&src);
        generic::RANK_STEPS.get()
    };
    for row in [
        ("let r: u8 = ", "k(1, ", "1", ")"),
        ("let r: u8? = ", "id(", "1", ")"),
        ("let r: u8? = ", "k(Some(1), ", "None", ")"),
    ] {
        assert!(steps(row, 1) > 0, "the counter should count the walk");
        assert_near_linear(steps(row, 20), steps(row, 40), steps(row, 60));
    }
    // Joins and operators rank their siblings too: an array in an array, an `if` in an `if`, an
    // operator in an operator. Each level recurses deeper than a call does, and 60 levels
    // overflow the stack of a debug test thread, so the depths are smaller.
    for row in [
        ("let r = ", "[1, ", "1", "].len()"),
        ("let r = ", "if c { x } else { ", "1", " }"),
        ("let r = ", "x + (", "1", ")"),
        ("let r = ", "{ 1 } + (", "x", ")"),
    ] {
        assert!(steps(row, 1) > 0, "the counter should count the walk");
        assert_near_linear(steps(row, 10), steps(row, 20), steps(row, 30));
    }
}

/// The declarations that the rows of the failed bound tests share. `J` is an interface that no type
/// conforms to, and `V` conforms to `K<i64>` only. `none` and `two` are calls that only their
/// context can bind.
const REJECT_DECLS: &str = "interface J {\n}\n\
    interface K<X> {\n}\n\
    class V: K<i64> {\n}\n\
    class Plain {\n}\n\
    fn none<T>() -> T? { None }\n\
    fn two<A, B>() -> Two<A, B> { panic(\"e\") }\n";

/// A row of `PartialRow`, with the declarations that it follows written before its own.
type SharedRow<'a> = (String, &'a str, &'a [(Code, &'a str)]);

/// `check_partial_rows`, with `shared` before the declarations of each row.
fn check_rows_after(shared: &str, rows: &[PartialRow<'_>]) {
    let rows: Vec<SharedRow<'_>> = rows
        .iter()
        .map(|&(decls, body, expected)| (format!("{shared}{decls}"), body, expected))
        .collect();
    let rows: Vec<PartialRow<'_>> = rows
        .iter()
        .map(|(decls, body, expected)| (decls.as_str(), *body, *expected))
        .collect();
    check_partial_rows(&rows);
}

/// The rows of `a_failed_bound_should_not_check_a_later_argument_against_the_rejected_type`.
const REJECT_ROWS: &[PartialRow<'static>] = {
    use Code::{Mismatch, NotConform};
    const P: &str = "Two.P(n";
    &[
        // The first argument binds `T` to `i64`. `I` has no conformance for it.
        (
            "fn h<T: I>(_p: Two<T, i64>, _a: T) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), W())",
            &[(NotConform, P)],
        ),
        (
            "fn h<T: I>(_p: Two<T, T.Item>, _a: T) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), W())",
            &[(NotConform, P)],
        ),
        (
            "fn h<T: I, U>(_p: Two<T, i64>, _a: Two<T, U>) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), Two.P(W(), 1))",
            &[(NotConform, P)],
        ),
        // The parameter is in an array, an optional and a nested call.
        (
            "fn h<T: I>(_p: Two<T, i64>, _a: [T]) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), [W(), W()])",
            &[(NotConform, P)],
        ),
        (
            "fn h<T: I>(_p: Two<T, i64>, _a: [[T]]) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), [[W()], [W()]])",
            &[(NotConform, P)],
        ),
        (
            "fn h<T: I>(_p: Two<T, i64>, _a: [T?]) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), [Some(W()), None])",
            &[(NotConform, P)],
        ),
        (
            "fn h<T: I>(_p: Two<T, i64>, _a: T?) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), Some(W()))",
            &[(NotConform, P)],
        ),
        (
            "fn h<T: I>(_p: Two<T, i64>, _a: T) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), id(W()))",
            &[(NotConform, P)],
        ),
        // Two arguments after it, and one that is wrong in its own right.
        (
            "fn h<T: I>(_p: Two<T, i64>, _a: T, _b: T) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), W(), \"s\")",
            &[(NotConform, P)],
        ),
        (
            "fn h<T: I>(_p: Two<T, i64>, _a: T, _b: bool) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), W(), 1)",
            &[(NotConform, P), (Mismatch, "1)")],
        ),
        // Each bound that fails is reported, and each parameter that has one is rejected.
        (
            "fn h<T: I, U: J>(_p: Two<T, U>, _a: T, _b: U) {}\n",
            "let n: i64 = 5\nh(Two.P(n, true), W(), false)",
            &[(NotConform, P), (NotConform, P)],
        ),
        (
            "fn h<T: I & J>(_p: Two<T, i64>, _a: T) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), W())",
            &[(NotConform, P), (NotConform, P)],
        ),
        (
            "fn h<T: I & J>(_p: Two<T, i64>, _a: T) {}\n",
            "h(Two.P(W(), 7), W())",
            &[(NotConform, "Two.P(W")],
        ),
        // A second bound of the rejected parameter waits for `U`. It is not checked when `U` binds.
        (
            "fn h<T: I & K<U>, U>(_a: T, _b: U) {}\n",
            "let a = 5\nlet b = \"s\"\nh(a, b)",
            &[(NotConform, "a")],
        ),
        // The bound names another parameter.
        (
            "fn h<T: K<U>, U>(_a: T, _b: U, _c: Two<T, U>) {}\n",
            "h(5, \"s\", Two.P(V(), \"t\"))",
            &[(NotConform, "Two.P(V")],
        ),
        // The type argument is written.
        (
            "fn h<T: I>(_p: Two<T, i64>, _a: T) {}\n",
            "let n: i64 = 5\nh<i64>(Two.P(n, 7), W())",
            &[(NotConform, "i64")],
        ),
        (
            "fn f<T: Eq>(_a: T, _b: T) {}\n",
            "f(Plain(), 5)",
            &[(NotConform, "Plain()")],
        ),
        // A method, a method of a generic class, a safe call, a construction and a variant.
        (
            "class Bx {\n    fn m<T: I>(self, _p: Two<T, i64>, _a: T) {}\n}\n",
            "let n: i64 = 5\nBx().m(Two.P(n, 7), W())",
            &[(NotConform, P)],
        ),
        (
            "class Gr<X> {\n    x: X\n    fn m<U: I>(self, _p: Two<U, i64>, _a: U) {}\n}\n",
            "let n: i64 = 5\nGr(x: true).m(Two.P(n, 7), W())",
            &[(NotConform, P)],
        ),
        (
            "class Sf {\n    fn m<T: I>(self, _p: Two<T, i64>, _a: T) {}\n}\n",
            "let n: i64 = 5\nlet o: Sf? = Sf()\no?.m(Two.P(n, 7), W())",
            &[(NotConform, P)],
        ),
        (
            "class Hold<T: I> {\n    p: Two<T, i64>\n    a: T\n}\n",
            "let n: i64 = 5\nHold(p: Two.P(n, 7), a: W())",
            &[(NotConform, P)],
        ),
        (
            "enum En<T: I> {\n    V(Two<T, i64>, T)\n}\n",
            "let n: i64 = 5\nEn.V(Two.P(n, 7), W())",
            &[(NotConform, P)],
        ),
    ]
};

/// A parameter whose bound fails is rejected: no argument after it is asked for the type that the
/// bound rejected, so the failed bound is the call's only error for that parameter.
#[test]
fn a_failed_bound_should_not_check_a_later_argument_against_the_rejected_type() {
    check_rows_after(REJECT_DECLS, REJECT_ROWS);
}

/// The rows of `an_argument_after_a_failed_bound_should_report_the_part_that_is_wrong`.
const REJECT_PART_ROWS: &[PartialRow<'static>] = {
    use Code::{Mismatch, NotConform};
    const P: &str = "Two.P(n";
    const S: &str = "\"s\"";
    &[
        // `U` is not rejected: the first argument that binds it fixes it.
        (
            "fn h<T: I, U>(_p: Two<T, T.Item>, _a: U, _b: Two<U, T.Item>) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), true, Two.P(\"s\", 3))",
            &[(NotConform, P), (Mismatch, S)],
        ),
        (
            "fn h<T: I, S: I, U>(_p: Two<T, T.Item>, _s: S, _a: U, _b: Two<U, S.Item>) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), W(), true, Two.P(\"s\", 2))",
            &[(NotConform, P), (Mismatch, S)],
        ),
        (
            "fn h<T: I, U>(_p: Two<T, i64>, _a: Two<T, U>, _b: U) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), Two.P(W(), 1), \"s\")",
            &[(NotConform, P), (Mismatch, S)],
        ),
        (
            "fn h<T: I, U>(_p: Two<T, i64>, _a: [Two<T, U>], _b: U) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), [Two.P(W(), 1), Two.P(W(), 2)], \"s\")",
            &[(NotConform, P), (Mismatch, S)],
        ),
        // The part that is not the rejected parameter must still meet the parameter's type.
        (
            "fn h<T: I>(_p: Two<T, i64>, _a: Two<T, bool>) {}\n",
            "let n: i64 = 5\nlet t = Two.P(W(), 1)\nh(Two.P(n, 7), t)",
            &[(NotConform, P), (Mismatch, "t)")],
        ),
        (
            "fn h<T: I>(_p: Two<T, i64>, _a: Two<T, bool>) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), mk(W(), 1))",
            &[(NotConform, P), (Mismatch, "1))")],
        ),
    ]
};

/// A rejected parameter meets every type. The other parts of the argument, and the other
/// parameters of the call, are still checked.
#[test]
fn an_argument_after_a_failed_bound_should_report_the_part_that_is_wrong() {
    check_rows_after(REJECT_DECLS, REJECT_PART_ROWS);
}

/// The rows of `a_call_that_waits_for_a_rejected_parameter_should_report_no_inference_error`.
const REJECT_WAIT_ROWS: &[PartialRow<'static>] = {
    use Code::NotConform;
    &[
        (
            "fn h<T: I>(_p: T, _a: T?) {}\n",
            "h(5, none())",
            &[(NotConform, "5")],
        ),
        (
            "fn h<T: I>(_p: T, _a: [T?]) {}\n",
            "h(5, [none()])",
            &[(NotConform, "5")],
        ),
        (
            "fn h<T: I>(_p: T, _a: Two<T, i64>) {}\n",
            "h(5, two())",
            &[(NotConform, "5")],
        ),
        (
            "fn h<T: I>(_p: T, _a: Two<T, T>) {}\n",
            "h(5, two())",
            &[(NotConform, "5")],
        ),
    ]
};

/// The context of a call that waits holds `REJECTED` for a rejected parameter. The call rejects its
/// own parameter there, and the failed bound is the cause, so the call adds no E0050.
#[test]
fn a_call_that_waits_for_a_rejected_parameter_should_report_no_inference_error() {
    check_rows_after(REJECT_DECLS, REJECT_WAIT_ROWS);
}

/// The rows of the test of a call that waits for another parameter, below.
const OTHER_WAIT_ROWS: &[PartialRow<'static>] = {
    use Code::{CannotInfer, NotConform};
    &[
        (
            "fn h<T: I, U>(_p: Two<T, i64>, _a: U?) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), none())",
            &[(NotConform, "Two.P(n"), (CannotInfer, "none()")],
        ),
        (
            "fn h<T: I, U>(_p: Two<T, i64>, _a: [U?]) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), [none()])",
            &[(NotConform, "Two.P(n"), (CannotInfer, "none()")],
        ),
    ]
};

/// The context of a call that waits for `U` holds `HOLE` for `U`, which no bound rejected. The
/// call cannot bind it, and nothing else reports that, so the call keeps its E0050 after a failed
/// bound of `T`.
#[test]
fn a_call_that_waits_for_another_parameter_should_still_report_its_own_inference_error() {
    check_rows_after(REJECT_DECLS, OTHER_WAIT_ROWS);
}

/// The rows of `a_rejected_part_of_a_context_should_reject_the_parameter_of_a_nested_call`.
const NESTED_REJECT_ROWS: &[PartialRow<'static>] = {
    use Code::NotConform;
    &[
        // A call that has not failed itself passes the context on: `id` gives it to `none`.
        (
            "fn h<T: I>(_p: T, _a: T?) {}\n",
            "h(5, id(none()))",
            &[(NotConform, "5")],
        ),
        (
            "class Gm {\n    fn m<X>(self, x: X) -> X { x }\n}\n\
             fn h<T: I>(_p: T, _a: T?) {}\n",
            "h(5, Gm().m(none()))",
            &[(NotConform, "5")],
        ),
        (
            "fn h<T: I>(_p: T, _a: [T?]) {}\n",
            "h(5, [id(none())])",
            &[(NotConform, "5")],
        ),
        (
            "fn h<T: I>(_p: T, _a: Two<T, i64>) {}\n",
            "h(5, id(two()))",
            &[(NotConform, "5")],
        ),
        (
            "fn h<T: I>(_p: Two<T, i64>, _a: T?) {}\n",
            "let n: i64 = 5\nh(Two.P(n, 7), id(none()))",
            &[(NotConform, "Two.P(n")],
        ),
        // A bare `T` is the whole context, so the call meets `REJECTED` itself. It rejects each
        // parameter that its result names, and meets a part that names none.
        (
            "fn h<T: I>(_p: T, _a: T) {}\n",
            "h(5, none())",
            &[(NotConform, "5")],
        ),
        (
            "fn arr<X>() -> [X] { panic(\"e\") }\nfn h<T: I>(_p: T, _a: T) {}\n",
            "h(5, arr())",
            &[(NotConform, "5")],
        ),
        (
            "fn h<T: I>(_p: T, _a: T) {}\n",
            "h(5, two())",
            &[(NotConform, "5")],
        ),
        (
            "fn bx<A, B>() -> Box<A, B> { panic(\"e\") }\nfn h<T: I>(_p: T, _a: T) {}\n",
            "h(5, bx())",
            &[(NotConform, "5")],
        ),
        (
            "fn two_i<A>() -> Two<A, i64> { panic(\"e\") }\nfn h<T: I>(_p: T, _a: T) {}\n",
            "h(5, two_i())",
            &[(NotConform, "5")],
        ),
        // `Some`, a variant and a safe call pass the context on to their parts.
        (
            "fn h<T: I>(_p: T, _a: T?) {}\n",
            "h(5, Some(none()))",
            &[(NotConform, "5")],
        ),
        (
            "fn h<T: I>(_p: T, _a: Two<T, i64>) {}\n",
            "let n = 5\nh(n, Two.P(none(), 1))",
            &[(NotConform, "n")],
        ),
        // A context that holds `REJECTED` inside, and no `HOLE`, rejects each parameter that a
        // call in an argument binds there: no type stands for the part, so the call reports
        // nothing for it.
        (
            "fn any<X>() -> X { panic(\"e\") }\nfn h<T: I>(_p: T, _a: [T]) {}\n",
            "h(5, any())",
            &[(NotConform, "5")],
        ),
        (
            "fn any<X>() -> X { panic(\"e\") }\nfn h<T: I>(_p: T, _a: Two<T, i64>) {}\n",
            "h(5, any())",
            &[(NotConform, "5")],
        ),
        (
            "fn any<X>() -> X { panic(\"e\") }\nfn h<T: I>(_p: T, _a: [[T]]) {}\n",
            "h(5, [any()])",
            &[(NotConform, "5")],
        ),
        (
            "fn h<T: I>(_p: T, _a: [T?]) {}\n",
            "h(5, id([none()]))",
            &[(NotConform, "5")],
        ),
        (
            "fn any<X>() -> X { panic(\"e\") }\nfn h<T: I>(_p: T, _a: [T]) {}\n",
            "h(5, dbg(any()))",
            &[(NotConform, "5")],
        ),
        (
            "class Sf {\n    fn m<X>(self, x: X?) -> X? { None }\n}\n\
             fn h<T: I>(_p: T, _a: [T?]?) {}\n",
            "let o: Sf? = Sf()\nh(5, o?.m(none()))",
            &[(NotConform, "5")],
        ),
        // `T` meets `HOLE` in one part of the callee's pattern and `REJECTED` in another: the
        // rejected part decides, whichever comes first.
        (
            "fn same<X>() -> Two<X, X> { panic(\"e\") }\n\
             fn h<T: I, U>(_p: T, _a: Two<[U], [T]>) {}\n",
            "h(5, same())",
            &[(NotConform, "5")],
        ),
        (
            "fn same<X>() -> Two<X, X> { panic(\"e\") }\n\
             fn h<T: I, U>(_p: T, _a: Two<[T], [U]>) {}\n",
            "h(5, same())",
            &[(NotConform, "5")],
        ),
        (
            "fn same<X>() -> Two<X, X> { panic(\"e\") }\n\
             fn h<T: I, U>(_p: T, _a: Two<U?, T?>) {}\n",
            "h(5, same())",
            &[(NotConform, "5")],
        ),
    ]
};

/// A context holds `REJECTED` where a parameter's bound failed. A call in an argument that gets it
/// rejects its own parameter at that place, however many calls the context passes through, and
/// reports nothing for it: the failed bound is the fault.
#[test]
fn a_rejected_part_of_a_context_should_reject_the_parameter_of_a_nested_call() {
    check_rows_after(REJECT_DECLS, NESTED_REJECT_ROWS);
}

/// The rows of `an_unbound_part_next_to_a_rejected_one_should_keep_its_inference_error`.
const UNBOUND_NEXT_ROWS: &[PartialRow<'static>] = {
    use Code::{CannotInfer, NotConform};
    &[
        // `B` of `two_p` sits at `U`, which no bound rejected.
        (
            "class P {}\n\
             fn two_p<B>() -> Two<P, B> { panic(\"e\") }\n\
             fn h<T: I, U>(_a: T, _b: Two<T, U>) {}\n",
            "h(P(), two_p())",
            &[(NotConform, "P()"), (CannotInfer, "two_p()")],
        ),
        (
            "fn h<T: I, U>(_p: T, _a: Two<T, U>?) {}\n",
            "h(5, dbg(none()))",
            &[(NotConform, "5"), (CannotInfer, "none()")],
        ),
        (
            "fn g<A, B>(_x: B?) -> A? { None }\n\
             fn h<T: I>(_p: T, _a: T?) {}\n",
            "h(5, g(none()))",
            &[(NotConform, "5"), (CannotInfer, "none()")],
        ),
        (
            "fn h<T: I, U>(_p: T, _a: U?) {}\n",
            "h(5, id(none()))",
            &[(NotConform, "5"), (CannotInfer, "none()")],
        ),
        (
            "fn h<T: I, U>(_p: T, _a: U?) {}\n",
            "h(5, none())",
            &[(NotConform, "5"), (CannotInfer, "none()")],
        ),
        // One call has a rejected parameter and an unbound one, in both orders: `A` is rejected
        // and `B` is not, or the reverse.
        (
            "fn h<T: I, U>(_p: T, _a: Two<T, U>) {}\n",
            "h(5, two())",
            &[(NotConform, "5"), (CannotInfer, "two()")],
        ),
        (
            "fn h<T: I, U>(_p: T, _a: Two<U, T>) {}\n",
            "h(5, two())",
            &[(NotConform, "5"), (CannotInfer, "two()")],
        ),
        (
            "fn h<T: I, U>(_p: T, _a: [Two<T, U>]) {}\n",
            "h(5, [two()])",
            &[(NotConform, "5"), (CannotInfer, "two()")],
        ),
        (
            "fn h<T: I, U>(_p: T, _a: Two<T, U>) {}\n",
            "h(5, id(two()))",
            &[(NotConform, "5"), (CannotInfer, "two()")],
        ),
        // `A` of `two` sits in `[T]`, a part that holds `REJECTED` and no `HOLE`; `B` sits at `U`.
        // The call keeps the E0050 of the parameter that is not rejected-only, whichever comes
        // first.
        (
            "fn h<T: I, U>(_p: T, _a: Two<[T], U>) {}\n",
            "h(5, two())",
            &[(NotConform, "5"), (CannotInfer, "two()")],
        ),
        (
            "fn h<T: I, U>(_p: T, _a: Two<U, [T]>) {}\n",
            "h(5, two())",
            &[(NotConform, "5"), (CannotInfer, "two()")],
        ),
        // A part that holds both holes is not rejected-only.
        (
            "fn h<T: I, U>(_p: T, _a: Two<[T], U>?) {}\n",
            "h(5, none())",
            &[(NotConform, "5"), (CannotInfer, "none()")],
        ),
        (
            "fn any<X>() -> X { panic(\"e\") }\nfn h<T: I, U>(_p: T, _a: [T], _b: U?) {}\n",
            "h(5, any(), none())",
            &[(NotConform, "5"), (CannotInfer, "none()")],
        ),
    ]
};

/// A part of a context that no bound rejected is `HOLE`, not `REJECTED`. A nested call that it
/// leaves with a parameter unbound still reports E0050, next to the failed bound. So does a call
/// that has a rejected parameter and an unbound one.
#[test]
fn an_unbound_part_next_to_a_rejected_one_should_keep_its_inference_error() {
    check_rows_after(REJECT_DECLS, UNBOUND_NEXT_ROWS);
}

/// The rows of `a_call_that_rejected_a_parameter_should_blame_no_argument_for_its_expected_type`.
const NO_BLAME_ROWS: &[PartialRow<'static>] = {
    use Code::NotConform;
    &[
        (
            "fn h<T: I, U>(p: Two<T, U>) -> Two<T, U> { p }\n",
            "let n: i64 = 5\nlet x: Two<Plain, bool> = h(Two.P(n, 7))",
            &[(NotConform, "Two.P(n")],
        ),
        (
            "fn h<T: I, U>(p: Two<T, U>) -> Two<T, U> { p }\n",
            "let n: i64 = 5\nlet x: Two<W, bool> = h(Two.P(n, 7))",
            &[(NotConform, "Two.P(n")],
        ),
    ]
};

/// The expected type does not meet the result of a call whose parameter `T` is rejected, as `U`
/// is `i64` and the expected type says `bool`. The failed bound is the fault: the call blames no
/// argument, and so reports the bound once.
#[test]
fn a_call_that_rejected_a_parameter_should_blame_no_argument_for_its_expected_type() {
    check_rows_after(REJECT_DECLS, NO_BLAME_ROWS);
}

/// Classes that conform to `K` for a type that holds the parameter of a bound: the arguments of
/// the bound name the parameter in an array, an optional, an enum and a class.
const WAIT_SHAPE_DECLS: &str = "class Va: K<[i64]> {\n}\n\
    class Vo: K<i64?> {\n}\n\
    class Vt: K<Two<i64, bool>> {\n}\n\
    class Vc: K<Box<i64, bool>> {\n}\n";

/// The rows of `a_bound_that_names_a_parameter_in_a_larger_type_should_wait_for_it`.
const WAIT_SHAPE_ROWS: &[PartialRow<'static>] = {
    use Code::NotConform;
    &[
        // The first argument binds `T`, and the bound waits for `U`, which the last binds.
        ("fn h<T: K<[U]>, U>(_a: T, _b: U) {}\n", "h(Va(), 1)", &[]),
        ("fn h<T: K<U?>, U>(_a: T, _b: U) {}\n", "h(Vo(), 1)", &[]),
        (
            "fn h<T: K<Two<U, bool>>, U>(_a: T, _b: U) {}\n",
            "h(Vt(), 1)",
            &[],
        ),
        (
            "fn h<T: K<Box<U, bool>>, U>(_a: T, _b: U) {}\n",
            "h(Vc(), 1)",
            &[],
        ),
        // The bound fails once `U` is bound, and is reported at the argument for `T`.
        (
            "fn h<T: K<[U]>, U>(_a: T, _b: U) {}\n",
            "h(Plain(), 1)",
            &[(NotConform, "Plain()")],
        ),
    ]
};

/// A bound waits for the first parameter that its arguments name, wherever they name it. It is
/// checked when the call binds that parameter, and not before.
#[test]
fn a_bound_that_names_a_parameter_in_a_larger_type_should_wait_for_it() {
    check_rows_after(
        &format!("{REJECT_DECLS}{WAIT_SHAPE_DECLS}"),
        WAIT_SHAPE_ROWS,
    );
}

/// The rows of `a_bound_that_names_an_associated_type_of_a_rejected_parameter_should_add_no_error`.
const REJECTED_BASE_ROWS: &[PartialRow<'static>] = {
    use Code::NotConform;
    &[(
        "fn h<T: I, U: K<T.Item>>(_p: Two<T, U>) {}\n",
        "h(Two.P(Plain(), V()))",
        &[(NotConform, "Two.P(Plain")],
    )]
};

/// One argument binds `T` and `U` together, and the bound of `T` fails. The bound of `U` names
/// `T.Item`, which has no type, as `Plain` has no conformance. The failed bound of `T` is the
/// only error: the bound of `U` has nothing to wait for.
#[test]
fn a_bound_that_names_an_associated_type_of_a_rejected_parameter_should_add_no_error() {
    check_rows_after(REJECT_DECLS, REJECTED_BASE_ROWS);
}

/// The rows of `a_failed_bound_should_come_before_a_mismatch_with_the_expected_type`.
const REJECT_EXPECTED_ROWS: &[PartialRow<'static>] = {
    use Code::NotConform;
    const P: &str = "Two.P(n";
    &[
        (
            "fn h<T: I>(_p: Two<T, i64>) -> T? { None }\n",
            "let n: i64 = 5\nlet x: W? = h(Two.P(n, 7))",
            &[(NotConform, P)],
        ),
        (
            "fn h<T: I>(_p: Two<T, T.Item>) -> T? { None }\n",
            "let n: i64 = 5\nlet x: W? = h(Two.P(n, 7))",
            &[(NotConform, P)],
        ),
        (
            "fn h<T: I>(_p: Two<T, i64>, a: T) -> T { a }\n",
            "let n: i64 = 5\nlet x: i64 = h(Two.P(n, 7), W())",
            &[(NotConform, P)],
        ),
        (
            "fn h<T: I>(_p: Two<T, i64>, a: [T]) -> [T] { a }\n",
            "let n: i64 = 5\nlet x: [W] = h(Two.P(n, 7), [W()])",
            &[(NotConform, P)],
        ),
        (
            "fn h<T: I>(_p: Two<T, i64>, a: T) -> Two<T, T> { Two.P(a, a) }\n",
            "let n: i64 = 5\nlet x: Two<W, W> = h(Two.P(n, 7), W())",
            &[(NotConform, P)],
        ),
        (
            "class Sf {\n    fn m<T: I>(self, _p: Two<T, i64>, _a: T) -> T? { None }\n}\n",
            "let n: i64 = 5\nlet o: Sf? = Sf()\nlet x: W? = o?.m(Two.P(n, 7), W())",
            &[(NotConform, P)],
        ),
    ]
};

/// An expected type does not bind the parameter of a call whose bound failed: the failed bound is
/// reported, and the mismatch with the expected type is not.
#[test]
fn a_failed_bound_should_come_before_a_mismatch_with_the_expected_type() {
    check_rows_after(REJECT_DECLS, REJECT_EXPECTED_ROWS);
}

/// The rows of `a_bound_that_the_expected_type_binds_should_give_one_error_when_it_fails`.
const EXPECTED_BOUND_ROWS: &[PartialRow<'static>] = {
    use Code::NotConform;
    &[
        // The expected type binds `T` for the whole call, and the bound fails at the call.
        (
            "fn mkb<T: I>() -> Two<T, i64>? { None }\n",
            "let x: Two<i64, i64>? = mkb()",
            &[(NotConform, "mkb()")],
        ),
    ]
};

/// A parameter that the expected type binds, and no argument, is checked at the call. When its
/// bound fails, the call reports E0154 alone, at the call.
#[test]
fn a_bound_that_the_expected_type_binds_should_give_one_error_when_it_fails() {
    check_rows_after(REJECT_DECLS, EXPECTED_BOUND_ROWS);
}

/// The rows of `a_bound_that_holds_should_leave_the_arguments_after_it_checked`.
const HELD_BOUND_ROWS: &[PartialRow<'static>] = {
    use Code::Mismatch;
    &[
        // `W` binds `T` first, so the pair is what is wrong.
        (
            "fn h<T: I>(_a: T, _p: Two<T, i64>) {}\n",
            "h(W(), Two.P(5, 7))",
            &[(Mismatch, "5")],
        ),
        // The receiver binds `T`, and no bound of the method is there to fail. The associated
        // type is still replaced.
        (
            "class Gi<T: I> {\n    x: T\n    fn m(self, _a: T.Item) {}\n}\n",
            "Gi(x: W()).m(5)\nGi(x: W()).m(\"s\")",
            &[(Mismatch, "\"s\"")],
        ),
        (
            "fn h<T: I>(_p: Two<T, i64>, _a: T) {}\n",
            "h(Two.P(W(), 7), W())\nh(Two.P(W(), 7), 5)",
            &[(Mismatch, "5)")],
        ),
    ]
};

/// A bound that holds, or one that no argument rejects, changes nothing for the arguments after
/// it.
#[test]
fn a_bound_that_holds_should_leave_the_arguments_after_it_checked() {
    check_rows_after(REJECT_DECLS, HELD_BOUND_ROWS);
}

/// An array literal whose element makes an optional is no optional itself: the call that holds it
/// meets an optional context by its payload, and wraps the value, so `T` is the array.
#[test]
fn an_array_literal_should_not_make_the_call_that_holds_it_optional() {
    infers(&[
        (
            "fn g() { let r: [i64?]? = id([Some(1)]) }",
            "id",
            &["[i64?]", "[i64?]"],
        ),
        (
            "fn g(c: bool) { let r: [i64?]? = id([if c { 1 } else { None }]) }",
            "id",
            &["[i64?]", "[i64?]"],
        ),
    ]);
}

/// An element that is a call of `dbg` with a label does not fit its declaration, so it keeps the
/// order of a call also when the array is the argument of a generic call: it is checked first,
/// with no expected type, and reports its own mistakes, not a mismatch with the other element.
#[test]
fn a_call_that_does_not_fit_should_be_checked_first_in_an_array_argument() {
    with_prelude();
    let src =
        with_generics("fn g() {\n    take_all([dbg(x: []), 1])\n}\nfn take_all<T>(xs: [T]) {}");
    codes(
        &src,
        &[
            (Code::LabeledArgument, (2, 19)),
            (Code::CannotInfer, (2, 22)),
        ],
    );
}
