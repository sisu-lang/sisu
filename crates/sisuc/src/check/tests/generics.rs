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

/// A generic class is constructed with type arguments only from Task 4 on, so its fields are
/// never read with none.
#[test]
#[should_panic(expected = "type arguments in expressions land in Task 4")]
fn constructing_a_generic_class_waits_for_type_arguments() {
    checked("class C<T> {\n    x: T?\n}\nfn main() {\n    let c = C(x: None)\n    let _ = c.x\n}");
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
    // Before a `.`, the list cannot be two comparisons, so there is nothing to compare.
    for (line, at) in [
        ("fn g(xs: [i64]) -> i64 { xs<i64>.len() }", (1, 26, "xs")),
        ("fn g(p: Point) -> i64 { p<i64>.x }", (1, 25, "p")),
    ] {
        let d = diagnostic(&with_decls(line), Code::TypeArguments, at, &[]);
        assert!(d.help().is_none(), "{line}");
    }
}

/// Comparisons misread as a call take the commas of the call they are an argument of, whose
/// count of arguments is then not reported, a variant's and a method's as a function's; a
/// call around that one, or around a block that holds it, still has its count reported.
#[test]
fn a_misread_comparison_hides_only_its_own_call_s_count() {
    let decls = "enum E { A(bool, bool) }\nclass K {\n    fn m(self, x: bool, y: bool) {}\n}\nfn g(x: i64) -> i64 { x }\nfn h(x: bool, y: bool) -> bool { x }\nfn main() {}";
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
    // A string or a comment that holds the name does not use it.
    let src = "fn main<T>() {\n    print(\"T\") // T\n}";
    let d = diagnostic(src, Code::MainSignature, (1, 8, "<T>"), &[]);
    assert_eq!(fixes(src, &d), [(true, vec![(1, 8, "<T>", String::new())])]);
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

/// A slot that cannot be built yet is scanned again only once all it waits for is built, so
/// an enum of `n` fields, each of an enum built at once, costs a few scans per field.
#[test]
fn the_can_be_built_fixed_point_scans_linearly() {
    let scans = |n: usize| {
        let names: Vec<String> = (0..n).map(|i| format!("E{i}")).collect();
        let decls = names.join(" { A }\nenum ");
        let src = format!(
            "enum {decls} {{ A }}\nenum Root {{ A({}) }}\nfn main() {{}}",
            names.join(", ")
        );
        crate::check::enums::SCANS.set(0);
        clean(&src);
        crate::check::enums::SCANS.get()
    };
    assert_near_linear(scans(4), scans(8), scans(12));
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
fn type_arguments_on_an_unknown_member() {
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
