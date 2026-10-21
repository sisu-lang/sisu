//! Generic declarations: the scopes of type parameters, type arguments in types and
//! expressions, what a value of a type parameter may do, and which generic enums can be built.

use super::*;

/// The classes the rows name, and an empty `main`, after the line a row tests.
const DECLS: &str = "class Cell<T> {\n    value: T\n    next: Cell<T>?\n}\nclass Stack<T> {\n    mut top: Cell<T>?\n}\nclass Point {\n    x: i64\n}\nfn main() {}";

/// `line` on line 1, then `DECLS`.
fn with_decls(line: &str) -> String {
    format!("{line}\n{DECLS}")
}

#[test]
fn generic_declarations_check_clean() {
    for src in [
        "fn id<T>(x: T) -> T { x }",
        "fn wrap<T>(x: T) -> [T] { [x] }",
        "fn first<T>(xs: [T]) -> T? { if xs.len() == 0 { return None }\n xs[0] }",
        "fn none<T>(x: T?) -> bool { x == None }",
        "enum Wrap<T> { W(T) }",
        "class Queue<T> {\n    mut front: Cell<T>?\n    fn peek(self) -> Cell<T>? { self.front }\n    fn empty(self) -> bool { self.front == None }\n}",
        "enum Tree<T> {\n    Leaf\n    Node(Tree<T>, T, Tree<T>)\n}",
        "class Holder<T> {\n    value: T\n    fn get(self) -> T { self.value }\n    fn pair<U>(self, u: U) -> [U] { [u] }\n}",
        "fn id<Point>(x: Point) -> Point { x }",
        "fn f(s: Stack<i64>, t: [Stack<[i64]>]) {}",
        "fn f(s: Stack<[i64?]>) {}",
        "fn f(s: Stack<i64>) -> Cell<i64>? { s.top }",
        "fn f<T>(n: i64) -> [T?] { [None; n] }",
        "fn g<T>(xs: [T]) -> [[T]] { [xs; 2] }",
        "fn g(a: i64, b: i64, c: i64, d: i64) { f((a < b), c > (d)) }\nfn f(x: bool, y: bool) {}",
    ] {
        clean(&with_decls(src));
    }
}

/// `tir` names each declaration's type parameters, a method's owner's before its own.
#[test]
fn type_parameters_reach_tir() {
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
fn values_of_parameter_types_spill() {
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
fn type_arguments_in_types() {
    let arguments = |at| vec![(Code::TypeArguments, at)];
    let argument = vec![(Code::TypeArgument, (1, 15))];
    for (line, expected) in [
        ("fn f(s: Stack) {}", arguments((1, 9))),
        ("fn f(s: Stack<i64, i64>) {}", arguments((1, 14))),
        ("fn f(p: Point<i64>) {}", arguments((1, 14))),
        ("fn f(s: Stack<unit>) {}", argument.clone()),
        ("fn f(s: Stack<never>) {}", argument.clone()),
        ("fn f(s: Stack<i64?>) {}", argument),
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
}

/// A non-generic name's list is deleted, in a type and in an expression alike.
#[test]
fn type_arguments_on_a_non_generic_name_are_removed() {
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
fn type_arguments_on_a_value() {
    let src = "fn g(a: i64, b: i64, c: i64, d: i64) { f(a < b, c > (d)) }\nfn f(x: bool) {}\nfn main() {}";
    let d = diagnostic(src, Code::TypeArguments, (1, 42, "a"), &[]);
    assert!(d.help().is_some());
    assert_eq!(d.fixes(), []);
    let src = with_decls("fn g(p: Point, a: i64, b: i64, c: i64) -> bool { p.x < a, b > (c) }");
    diagnostic(&src, Code::TypeArguments, (1, 50, "p.x"), &[]);
}

#[test]
fn values_of_parameter_types_have_no_operators() {
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
fn is_on_a_parameter_has_no_help() {
    let src = with_decls("fn f<T>(a: T, b: T) -> bool { a is b }");
    let d = diagnostic(&src, Code::IsNeedsObjects, (1, 31, "a"), &[]);
    assert!(d.help().is_none());
}

#[test]
fn main_takes_no_type_parameters() {
    let src = "fn main<T>() {}";
    let d = diagnostic(src, Code::MainSignature, (1, 8, "<T>"), &[]);
    assert_eq!(fixes(src, &d), [(true, vec![(1, 8, "<T>", String::new())])]);
    fix_removes_its_code(src, &d);
    // The fix would leave `T` naming nothing.
    let src = "fn main<T>() { let x: T? = None }";
    let d = diagnostic(src, Code::MainSignature, (1, 8, "<T>"), &[]);
    assert_eq!(d.fixes(), []);
}

#[test]
fn type_parameter_names() {
    for (src, expected) in [
        ("fn f<i64>() {}", vec![(Code::BuiltInName, (1, 6))]),
        ("fn f<T, T>() {}", vec![(Code::DefinedTwice, (1, 9))]),
        (
            "class Holder<T> {\n    value: T\n    fn get<T>(self) -> T { self.value }\n}",
            vec![(Code::DefinedTwice, (3, 12))],
        ),
    ] {
        codes(&format!("{src}\nfn main() {{}}"), &expected);
    }
}

/// Which generic enums can be built depends on which of their arguments can: `Two<Expr>` only
/// if `Expr` can, and `Nest<T>` needs a `Nest<[T]>`, which needs what it does.
#[test]
fn generic_enums_that_cannot_be_built() {
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
        (
            "enum Nest<T> { A(Nest<[T]>) }",
            vec![(Code::NeverConstructed, (1, 6))],
        ),
    ] {
        codes(&format!("{src}\nfn main() {{}}"), &expected);
    }
}

/// A list after a name that takes none is reported at the list in every expression form, and
/// one after a value at the value.
#[test]
fn type_arguments_in_expressions() {
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
