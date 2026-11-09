//! A type parameter or associated type whose bound failed is unknown in each body, as one whose
//! closure is too wide is: the errors that a bound could remove go, and the errors that are
//! true for any bound stay (S31).

use super::*;
use Code::{
    ArgumentCount, BraceOnNewLine, CannotCompare, CannotIndex, CoalesceNotOptional,
    ConformanceBody, Expected, ForNeedsArray, Mismatch, NoMember, NotAnInterface, ParamLacks,
    SafeAccessNotOptional, TypeArguments, UnknownType,
};

/// The items that every program of S31 starts with.
const ITEMS: &str = "interface Y {\n    fn y(self) -> i64\n}\nclass Box2<U: Y> {\n    v: U\n}\nfn needs<U: Y>(u: U) -> i64 { 1 }\nfn int(a: i64) -> i64 { a }\n";

/// Asserts that the errors of `src`, in the order of their spans, have the codes `expected`.
/// A warning is out: the tables list errors. A failure names `row`.
fn error_codes(row: &str, src: &str, expected: &[Code]) {
    let (_, ds) = crate::front_end::checked_with(PRELUDE_SOURCE, src);
    let found: Vec<Code> = ds
        .iter()
        .filter(|d| d.severity() == Severity::Error)
        .map(Diagnostic::code)
        .collect();
    assert_eq!(found, expected, "row `{row}` of\n{src}");
}

/// `ITEMS`, then `form`, then `main`.
fn program(form: &str) -> String {
    format!("{ITEMS}{form}fn main() {{}}\n")
}

/// The 21 uses of S31's first table, with the codes that they keep after the error of the bound.
const USES: [(&str, &[Code]); 21] = [
    ("x.zz()", &[]),
    ("x.y()", &[]),
    ("let _a = x.zz().w", &[]),
    ("let _a = x == x", &[]),
    ("let _a = x < x", &[]),
    ("let _a = [x] == [x]", &[]),
    ("let _a = needs(x)", &[]),
    ("let _a: Box2<T>? = None", &[]),
    ("let _a: T.Item? = None", &[]),
    ("x?.y()", &[SafeAccessNotOptional]),
    ("let _a = x.f", &[NoMember]),
    ("let _a = x + x", &[ParamLacks]),
    ("let _a = -x", &[ParamLacks]),
    ("let _a = x == 1", &[CannotCompare]),
    ("let _a = !x", &[Mismatch]),
    ("let _a = x[0]", &[CannotIndex]),
    ("for _a in x {}", &[ForNeedsArray]),
    ("let _a = i64(x)", &[Mismatch]),
    ("let _a: i64 = x", &[Mismatch]),
    ("let _a = x ?? 1", &[CoalesceNotOptional]),
    ("print(x)", &[]),
];

/// S31's first table for the bound `bound`, declared by `decl`: the error `code` of the bound,
/// and then the code of each use.
fn each_use(decl: &str, bound: &str, code: Code) {
    for (form, kept) in USES {
        let src = program(&format!(
            "{decl}fn g<T: {bound}>(x: T) {{\n    {form}\n}}\n"
        ));
        let want: Vec<Code> = std::iter::once(code).chain(kept.iter().copied()).collect();
        error_codes(form, &src, &want);
    }
}

#[test]
fn an_unknown_bound_should_leave_only_its_error_at_each_use() {
    each_use("", "Missing", UnknownType);
}

#[test]
fn a_bound_that_did_not_parse_should_leave_only_its_error_at_each_use() {
    each_use("interface Z\n{\n    fn zz(self)\n}\n", "Z", BraceOnNewLine);
}

#[test]
fn a_class_bound_should_leave_only_its_error_at_each_use() {
    each_use("class K {}\n", "K", NotAnInterface);
}

#[test]
fn a_bound_with_a_failed_argument_should_leave_only_its_error_at_each_use() {
    each_use("interface W<A> {}\n", "W<Missing>", UnknownType);
}

/// `Q` with `type A`, and a function that uses it.
const Q_WITH_TYPE: &str =
    "interface Q {\n    type A\n    fn a(self) -> Self.A\n}\nfn q<U: Q>(u: U) -> U.A { u.a() }\n";

/// The rows 1-18 and 30-38 of S31's second table: the form, and the codes of the output.
const KINDS: [(&str, &[Code]); 27] = [
    (
        "fn g<T: Y & Missing>(x: T) { print(x.y() + true) }",
        &[UnknownType],
    ),
    ("fn g<T: Y & Missing>(x: T) { x.zz() }", &[UnknownType]),
    ("fn g<T: Missing>(xs: [T]) { xs[0].zz() }", &[UnknownType]),
    ("fn g<T: Missing>(o: T?) { o?.zz() }", &[UnknownType]),
    ("fn g<T: Missing>(o: T?) -> bool { o == o }", &[UnknownType]),
    (
        "class C<T: Missing> {\n    v: T\n    fn m(self) { self.v.zz() }\n}",
        &[UnknownType],
    ),
    (
        "enum E<T: Missing> {\n    A(T)\n    fn m(self, x: T) { x.zz() }\n}",
        &[UnknownType],
    ),
    (
        "class C {\n    fn m<T: Missing>(self, x: T) { x.zz() }\n}",
        &[UnknownType],
    ),
    (
        "class B<T> {\n    v: T\n}\nextend<T: Missing> B<T> {\n    fn m(self) { self.v.zz() }\n}",
        &[UnknownType],
    ),
    (
        "interface I {\n    type A: Missing\n    fn a(self) -> Self.A\n}\nfn g<T: I>(x: T) { x.a().zz() }",
        &[UnknownType],
    ),
    (
        "interface I {\n    type A: Missing\n    fn a(self) -> Self.A\n}\nfn g<T: I>(x: T) -> bool { x.a() == x.a() }",
        &[UnknownType],
    ),
    (
        "fn g<T: Missing, V>(x: T, v: V) { v.zz() }",
        &[UnknownType, NoMember],
    ),
    ("fn g<T: Y<i64>>(x: T) { x.y() }", &[TypeArguments]),
    (
        "fn g<T: Y<i64>>(x: T) { x.zz() }",
        &[TypeArguments, NoMember],
    ),
    ("fn g<T: Missing>(x: Box2<T>) {}", &[UnknownType]),
    (
        "interface W<A: Y> {}\nfn g<T: Missing, U: W<T>>(x: U) {}",
        &[UnknownType],
    ),
    (
        "interface W<A> {\n    fn w(self) -> A\n}\nfn g<T: W>(x: T) { x.w() }",
        &[TypeArguments],
    ),
    (
        "interface W<A> {\n    fn w(self) -> A\n}\nfn g<T: W<i64, i64>>(x: T) { x.w() }",
        &[TypeArguments],
    ),
    (
        "class B3<T: Missing> {\n    v: T\n}\nfn f() {\n    let _a = B3(v: 1)\n}",
        &[UnknownType],
    ),
    (
        "interface P {\n    fn m<U: Missing>(self, u: U)\n}\nclass C: P {\n    fn m<U>(self, u: U) {}\n}",
        &[UnknownType],
    ),
    (
        "fn g<A: Missing>(b: A) { let _v: i64 = q(b) }",
        &[UnknownType, Mismatch],
    ),
    (
        "fn g<A: Missing>(b: A) { q(b).zz() }",
        &[UnknownType, NoMember],
    ),
    ("fn g<T>(x: T) { x.zz() }", &[NoMember]),
    (
        "fn g<T: Missing>(x: T) {}\nfn f() { g(1, 2) }",
        &[UnknownType, ArgumentCount],
    ),
    (
        "fn g<T: Missing>(x: T) -> T { x }\nfn f() { g(1).zz() }",
        &[UnknownType, NoMember],
    ),
    (
        "fn g<T: Missing>(x: T) -> T { x }\nfn f() { g(1) + true }",
        &[UnknownType, Mismatch],
    ),
    (
        "class Q: Missing {}\nfn g(q: Q) { q.other() }",
        &[UnknownType, NoMember],
    ),
];

/// The probes of the plan: the form, and the codes of the output.
const PROBES: [(&str, &[Code]); 7] = [
    // A member bound beside a failed one.
    (
        "interface Conv<A> {\n    fn c(self) -> A\n}\nfn g<T: Missing & Conv<T.Item>>(x: T) {}\n",
        &[UnknownType],
    ),
    // A failed bound beside a known one.
    (
        "interface K {}\nfn g<T: K & Missing>(x: T) -> bool { x == x }\n",
        &[UnknownType],
    ),
    // A bound reads a parameter in the wave after the wave of the bound that failed.
    (
        "interface Holds<A> {\n    type Item\n}\ninterface Conv<A> {}\nfn g<T: Holds<R.Item>, U: Conv<T.Item>, R: Missing>(t: T, u: U, r: R) {}\n",
        &[UnknownType],
    ),
    (
        "interface Holds<A> {\n    type Item\n}\ninterface Conv<A> {}\nfn g<T: Y & Holds<R.Item>, U: Conv<T.Item>, R: Missing>(t: T, u: U, r: R) {}\n",
        &[UnknownType],
    ),
    // A bound reads a parameter in the wave of the bound that failed, after it.
    (
        "interface Holds<A> {\n    type Item\n}\ninterface Conv<A> {}\nfn g<T: Holds<R.Item>, V: Conv<T.Name>, R: Missing>(t: T, v: V, r: R) {}\n",
        &[UnknownType],
    ),
    // A requirement keeps its old cascade.
    (
        "interface P {\n    fn m<U: Y>(self, u: U)\n}\nclass C: P {\n    fn m<U: Missing>(self, u: U) {}\n}\n",
        &[ConformanceBody, UnknownType],
    ),
    // A requirement that did not parse leaves its interface without it.
    (
        "interface Z {\n    fn zz(self) ->\n}\nfn g<T: Z>(x: T) { x.zz() }\n",
        &[Expected, NoMember],
    ),
];

#[test]
fn a_failed_bound_should_reach_each_kind_of_parameter() {
    for (form, want) in KINDS.iter().chain(&PROBES) {
        // The two rows of `Q` use its items; the others use none of them.
        let form = if form.contains("q(b)") {
            format!("{Q_WITH_TYPE}{form}\n")
        } else if form.ends_with('\n') {
            (*form).to_string()
        } else {
            format!("{form}\n")
        };
        error_codes(&form, &program(&form), want);
    }
}
