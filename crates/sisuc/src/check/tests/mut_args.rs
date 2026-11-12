//! `mut` arguments: the place, its type, the overlap of two places, the `MutCall` node, and the
//! warnings of a local that is passed `mut`.

use super::*;
use crate::check::mut_args::{MUT_KEY_STEPS, RootKey, Step};
use crate::tir::LocalId;

/// A class with one `mut` array field.
const BOARD: &str = "class Board {\n    mut cells: [i64]\n}\n";

/// A function that changes the array it is passed.
const SET: &str = "fn set(mut xs: [i64]) {\n    xs[0] = 9\n}\n";

/// The one diagnostic of `src`, which must be an error of `code`.
fn only(src: &str, code: Code) -> Diagnostic {
    let (program, mut ds) = checked(src);
    assert_eq!(ds.len(), 1, "{ds:?} in {src}");
    let d = ds.remove(0);
    assert_eq!(d.code(), code, "{src}");
    assert!(program.is_none(), "an error withholds the `tir`");
    d
}

/// GIVEN `fn f(mut a: i64, n: i64, mut b: i64)`, `fn change(mut i: i64) -> i64`, `let mut xs`
///   and `let mut i`
/// WHEN `main` calls `f(mut xs[i], change(mut i), mut xs[i])`
/// THEN the call checks clean: a `let mut` index can change, so it is no overlap
#[test]
fn let_mut_index_should_not_be_passed_twice_error() {
    let src = "fn f(mut a: i64, n: i64, mut b: i64) {\n    a = 3\n    b = 4\n}\n\
        fn change(mut i: i64) -> i64 {\n    i = 1\n    0\n}\n\
        fn main() {\n    let mut xs = [0, 0]\n    let mut i = 0\n    \
        f(mut xs[i], change(mut i), mut xs[i])\n    print(xs)\n}";
    codes(src, &[]);
}

/// GIVEN `two(mut a: [i64], mut b: [i64])` and two names `b1` and `c` of one `Board`
/// WHEN `main` calls `two(mut b1.cells, mut c.cells)`
/// THEN the call checks clean: the objects may differ
#[test]
fn places_through_two_objects_should_check_clean() {
    let src = format!(
        "{BOARD}fn two(mut a: [i64], mut b: [i64]) {{\n    a[0] = 1\n    b[0] = 2\n}}\n\
        fn main() {{\n    let b1 = Board(cells: [0])\n    let c = b1\n    \
        two(mut b1.cells, mut c.cells)\n    print(b1.cells)\n}}"
    );
    codes(&src, &[]);
}

/// GIVEN two `mut` fields of one object
/// WHEN a call passes both, through `self` and through a local
/// THEN the call checks clean: two fields of an object do not overlap
#[test]
fn two_fields_of_one_object_should_not_overlap() {
    let src = "fn swap(mut a: [i64], mut b: [i64]) {\n    let t = a\n    a = b\n    b = t\n}\n\
        class Pair {\n    mut a: [i64]\n    mut b: [i64]\n    \
        fn flip(self) {\n        swap(mut self.a, mut self.b)\n    }\n}\n\
        class P {\n    mut x: [i64]\n    mut y: [i64]\n}\n\
        fn main() {\n    let p = P(x: [1], y: [2])\n    swap(mut p.x, mut p.y)\n    \
        let q = Pair(a: [1], b: [2])\n    q.flip()\n    print(p.x)\n    print(q.a)\n}";
    codes(src, &[]);
}

/// GIVEN a class `P` and a class `Inner` held by a class `Outer`
/// WHEN a call passes a class place and a place inside it, in either order
/// THEN the call checks clean: "holds" stops at a class reference, and both store-backs land
#[test]
fn place_inside_a_class_place_should_check_clean() {
    let src = "class P {\n    mut x: [i64]\n}\n\
        class Inner {\n    mut cells: [i64]\n}\n\
        class Outer {\n    mut inner: Inner\n}\n\
        fn a(mut p: P, mut xs: [i64]) {\n    xs[0] = 1\n    p = P(x: xs)\n}\n\
        fn b(mut xs: [i64], mut p: P) {\n    xs[0] = 1\n    p = P(x: xs)\n}\n\
        fn c(mut i: Inner, mut xs: [i64]) {\n    xs[0] = 1\n    i = Inner(cells: xs)\n}\n\
        fn d(mut xs: [i64], mut i: Inner) {\n    xs[0] = 1\n    i = Inner(cells: xs)\n}\n\
        fn main() {\n    let mut p = P(x: [1])\n    a(mut p, mut p.x)\n    b(mut p.x, mut p)\n    \
        let o = Outer(inner: Inner(cells: [1]))\n    c(mut o.inner, mut o.inner.cells)\n    \
        d(mut o.inner.cells, mut o.inner)\n    print(p.x)\n    print(o.inner.cells)\n}";
    codes(src, &[]);
}

/// GIVEN an array of a class `Point`, whose elements live in the array
/// WHEN a call passes the array and one of its elements, in either order
/// THEN each is E0171; so is an element of an array of arrays, passed before the array
#[test]
fn element_of_a_class_array_should_be_inside() {
    let rows = [
        (
            "ps, ps[0]",
            "fn f(mut a: [Point], mut b: Point) {\n    b = Point(x: 1)\n    a[0] = b\n}\n\
            fn main() {\n    let mut ps = [Point(x: 1)]\n    f(mut ps, mut ps[0])\n    print(ps[0].x)\n}",
        ),
        (
            "ps[0], ps",
            "fn f(mut a: Point, mut b: [Point]) {\n    a = Point(x: 1)\n    b[0] = a\n}\n\
            fn main() {\n    let mut ps = [Point(x: 1)]\n    f(mut ps[0], mut ps)\n    print(ps[0].x)\n}",
        ),
        (
            "xs[0], xs",
            "fn f(mut a: [i64], mut b: [[i64]]) {\n    a[0] = 1\n    b[0] = a\n}\n\
            fn main() {\n    let mut xs = [[1]]\n    f(mut xs[0], mut xs)\n    print(xs)\n}",
        ),
    ];
    for (row, body) in rows {
        let src = format!("class Point {{\n    mut x: i64\n}}\n{body}");
        let d = only(&src, Code::MutPlaceTwice);
        // The fixtures `E0171-inside*` pin the wording.
        assert_eq!(d.secondary().len(), 1, "row `{row}`");
    }
}

/// GIVEN a call that passes the field of two temporaries, `mk().cells` twice
/// WHEN it is checked
/// THEN it checks clean: each call makes its own object
#[test]
fn two_temporary_roots_should_check_clean() {
    let src = format!(
        "{BOARD}fn mk() -> Board {{\n    Board(cells: [0])\n}}\n\
        fn f(mut a: [i64], mut b: [i64]) {{\n    a[0] = 1\n    b[0] = 2\n}}\n\
        fn main() {{\n    f(mut mk().cells, mut mk().cells)\n}}"
    );
    codes(&src, &[]);
}

/// GIVEN `swap(mut a: i64, mut b: i64)` and two `let` index locals `i` and `j`
/// WHEN `main` calls `swap(mut xs[i], mut xs[j])`
/// THEN the call checks clean: `i` and `j` may differ
#[test]
fn different_index_locals_should_check_clean() {
    let src = "fn swap(mut a: i64, mut b: i64) {\n    let t = a\n    a = b\n    b = t\n}\n\
        fn main() {\n    let mut xs = [1, 2]\n    let i = 0\n    let j = 1\n    \
        swap(mut xs[i], mut xs[j])\n    print(xs)\n}";
    codes(src, &[]);
}

/// The type of the one `MutCall` in the function `name` of `program`.
fn mut_call_type(program: &tir::Program, name: &str) -> Type {
    let function = program
        .functions
        .iter()
        .find(|f| f.name == name)
        .expect("the program should have the function");
    let mut all = Vec::new();
    function.body.exprs(&mut all);
    let mut calls = all
        .into_iter()
        .filter(|e| matches!(e.kind, tir::ExprKind::MutCall { .. }));
    let call = calls.next().expect("the function should hold a `MutCall`");
    assert!(calls.next().is_none(), "one `MutCall` only");
    call.ty
}

/// GIVEN `fn replace_with<T>(mut place: T, with: T) -> T` and `let mut x: i64? = None`
/// WHEN `main` calls `replace_with(mut x, 5)`
/// THEN the call checks clean, and its type is `i64?`: the `mut` argument fixes `T`
#[test]
fn generic_mut_argument_should_fix_the_type_parameter() {
    let src = "fn replace_with<T>(mut place: T, with: T) -> T {\n    let old = place\n    \
        place = with\n    old\n}\n\
        fn main() {\n    let mut x: i64? = None\n    let old = replace_with(mut x, 5)\n    \
        print(old ?? 0)\n    print(x ?? 0)\n}";
    let (program, ds) = checked(src);
    assert!(ds.is_empty(), "{ds:?}");
    let ty = mut_call_type(&program.expect("no errors"), "main");
    assert_eq!(ty, Type::optional(Type::I64));
}

/// GIVEN `fn clear(mut n: i64?)` and `let mut n = 1`
/// WHEN `main` calls `clear(mut n)`
/// THEN it is E0170 and nothing else: the `i64` place is not promoted to `i64?`
#[test]
fn mut_argument_should_not_promote() {
    let src = "fn clear(mut n: i64?) {\n    n = None\n}\n\
        fn main() {\n    let mut n = 1\n    clear(mut n)\n    print(n)\n}";
    codes(src, &[(Code::MutArgumentType, (6, 15))]);
}

/// The codes of the diagnostics of `src`, in order.
fn found_codes(src: &str) -> Vec<Code> {
    diags(src).iter().map(Diagnostic::code).collect()
}

/// GIVEN a generic callee whose `mut` parameter is `T?`, `[T?]` or an associated type, a method
///   of a generic class, a bound method, an `any I` method and a call with explicit type
///   arguments
/// WHEN a call passes a place that the parameter would promote to an optional, or a place of
///   another type
/// THEN it is E0170 and nothing else: after inference the type of the place is the parameter's
#[test]
fn generic_mut_argument_should_not_promote() {
    let assoc = "interface I {\n    type Item\n    fn get(self) -> Self.Item\n}\n\
        class C: I {\n    n: i64\n    type Item = i64\n    fn get(self) -> i64 {\n        self.n\n    }\n}\n\
        fn set<T: I>(c: T, mut x: T.Item) {\n    x = c.get()\n}\n";
    let put = "interface P {\n    fn put(self, mut x: i64?)\n}\n\
        class D: P {\n    n: i64\n    fn put(self, mut x: i64?) {\n        x = None\n    }\n}\n";
    let rows = [
        (
            "T?",
            "fn clr<T>(mut x: T?) {\n    x = None\n}\n\
            fn main() {\n    let mut n = 1\n    clr(mut n)\n    print(n)\n}"
                .to_owned(),
        ),
        (
            "T? with explicit type arguments",
            "fn clr<T>(mut x: T?) {\n    x = None\n}\n\
            fn main() {\n    let mut n = 1\n    clr<i64>(mut n)\n    print(n)\n}"
                .to_owned(),
        ),
        (
            "[T?]",
            "fn clr<T>(mut x: [T?]) {\n    x = [None]\n}\n\
            fn main() {\n    let mut n = [1]\n    clr(mut n)\n    print(n)\n}"
                .to_owned(),
        ),
        (
            "[T]?",
            "fn clr<T>(mut x: [T]?) {\n    x = None\n}\n\
            fn main() {\n    let mut n = [1]\n    clr(mut n)\n    print(n)\n}"
                .to_owned(),
        ),
        (
            "T??",
            "fn clr<T>(mut x: T??) {\n    x = None\n}\n\
            fn main() {\n    let mut n: i64? = 1\n    clr(mut n)\n    print(n ?? 0)\n}"
                .to_owned(),
        ),
        (
            "method of a generic class",
            "class S<T> {\n    n: i64\n    fn put(self, mut x: T?) {\n        x = None\n    }\n}\n\
            fn main() {\n    let s = S<i64>(n: 1)\n    let mut a = 1\n    s.put(mut a)\n    print(a)\n}"
                .to_owned(),
        ),
        (
            "bound method",
            format!(
                "{put}fn run<T: P>(t: T) {{\n    let mut a = 1\n    t.put(mut a)\n    print(a)\n}}\n\
                fn main() {{\n    run(D(n: 1))\n}}"
            ),
        ),
        (
            "any I method",
            format!(
                "{put}fn main() {{\n    let f: any P = D(n: 1)\n    let mut a = 1\n    \
                f.put(mut a)\n    print(a)\n}}"
            ),
        ),
        (
            "associated type",
            format!(
                "{assoc}fn main() {{\n    let mut x = \"wrong\"\n    set(C(n: 2), mut x)\n    print(x)\n}}"
            ),
        ),
        (
            "associated type with explicit type arguments",
            format!(
                "{assoc}fn main() {{\n    let mut x = \"wrong\"\n    set<C>(C(n: 2), mut x)\n    print(x)\n}}"
            ),
        ),
    ];
    for (row, src) in rows {
        assert_eq!(found_codes(&src), [Code::MutArgumentType], "row `{row}`");
    }
    // The right type is no error, also through an associated type.
    let src = format!(
        "{assoc}fn main() {{\n    let mut x = 1\n    set(C(n: 2), mut x)\n    print(x)\n}}"
    );
    codes(&src, &[]);
}

/// GIVEN a `mut` argument `mut self` or `mut self[0..1]` outside a method
/// WHEN it is checked
/// THEN it is E0041 only: there is no `self` to pass, so no E0169
#[test]
fn mut_self_outside_a_method_should_be_reported_once() {
    for place in ["self", "self[0..1]"] {
        let src =
            format!("fn set(mut x: i64) {{\n    x = 1\n}}\nfn main() {{\n    set(mut {place})\n}}");
        assert_eq!(
            found_codes(&src),
            [Code::SelfOutsideMethod],
            "place `{place}`"
        );
    }
}

/// GIVEN a `mut` argument that is a literal that needs its context, or any other temporary
/// WHEN it is checked in a program with no other error
/// THEN it is E0169: an unknown type with no error before it is no failure
#[test]
fn mut_argument_that_needs_its_context_should_be_reported_as_no_place() {
    for value in ["[]", "None", "[[]]", "mk()", "1"] {
        let src = format!(
            "{SET}fn mk() -> [i64] {{\n    [1]\n}}\nfn main() {{\n    set(mut {value})\n}}"
        );
        let found = found_codes(&src);
        assert!(
            found.contains(&Code::MutArgumentNotPlace),
            "`{value}`: {found:?}"
        );
    }
}

/// GIVEN a `mut` argument that is a range index, or the range of `self`, and whose expression
///   has an error
/// WHEN it is checked
/// THEN the error stands alone: the argument is not also reported as no place, and its label
///   does not guess the kind of its range
#[test]
fn mut_argument_with_a_failed_expression_should_not_be_reported_as_no_place() {
    let class = "class B {\n    mut s: string\n}\nfn set(mut x: string) {\n    x = \"a\"\n}\n";
    let rows = [
        (
            "range with a bad bound",
            "let mut s = \"abc\"\n    set(mut s[0..true])",
        ),
        (
            "range through `?.`",
            "let b: B? = B(s: \"a\")\n    set(mut b?.s[0..1])",
        ),
        ("range of an unknown name", "set(mut nope[0..1])"),
        ("range of `self`", "set(mut self[0..1])"),
        (
            "range of a string that failed to concatenate",
            "let s = \"a\" + 1\n    set(mut s[0..1])",
        ),
        (
            "range of a name that failed to bind",
            "let s = nope\n    set(mut s[0..1])",
        ),
    ];
    for (row, stmt) in rows {
        let src = format!("{class}fn main() {{\n    {stmt}\n}}");
        let found = found_codes(&src);
        assert!(!found.is_empty(), "row `{row}`: no error");
        assert!(
            !found.contains(&Code::MutArgumentNotPlace),
            "row `{row}`: {found:?}"
        );
    }
}

/// GIVEN a `mut` argument that is a range of a `string`, or of an element of an array, whose
///   `let` has a declared type and a value that fails
/// WHEN it is checked
/// THEN both errors stand: the declared type stays (#470), so the range is a known `string` range
#[test]
fn mut_argument_range_of_an_annotated_let_with_a_failed_value_should_be_no_place() {
    let head = "fn set(mut x: string) {\n    x = \"a\"\n}\nfn main() {\n";
    let rows = [
        ("string", "let mut s: string = nope\n    set(mut s[0..1])"),
        (
            "array element",
            "let s: [string] = nope\n    set(mut s[0][0..1])",
        ),
    ];
    for (row, stmt) in rows {
        let found = found_codes(&format!("{head}    {stmt}\n}}"));
        assert!(
            found.contains(&Code::BindingNotFound),
            "row `{row}`: {found:?}"
        );
        assert!(
            found.contains(&Code::MutArgumentNotPlace),
            "row `{row}`: {found:?}"
        );
    }
}

/// GIVEN `fn g(mut xs: [i64])` and `fn h(xs: [i64])`
/// WHEN the `tir` is printed
/// THEN the `mut` parameter shows its `mut`, as a `let mut` local does, and the other does not
#[test]
fn mut_parameter_should_print_with_its_mut() {
    let printed = lowered(
        "fn g(mut xs: [i64]) {\n    xs[0] = 1\n}\nfn h(xs: [i64]) {\n    print(xs)\n}\nfn main() {}",
    );
    assert!(printed.contains("(fn g ((mut xs#0 [i64]))"), "{printed}");
    assert!(printed.contains("(fn h ((xs#0 [i64]))"), "{printed}");
}

/// GIVEN `fn f(mut xs: [i64])` whose body assigns `xs` whole and writes an element
/// WHEN it is checked
/// THEN it checks clean, and the same body with a plain `xs` is E0001 twice
#[test]
fn mut_parameter_should_be_a_variable() {
    codes(
        "fn f(mut xs: [i64]) {\n    xs = []\n    xs[0] = 1\n}\nfn main() {}",
        &[],
    );
    let plain = diags("fn f(xs: [i64]) {\n    xs = []\n    xs[0] = 1\n}\nfn main() {}");
    let found: Vec<_> = plain.iter().map(Diagnostic::code).collect();
    assert_eq!(found, [Code::AssignToImmutable, Code::AssignToImmutable]);
}

/// GIVEN one callee with a `mut` parameter for each way a call reaches a written callee
/// WHEN a call passes it one `mut` argument
/// THEN the printed `tir` holds a `mut_call`
#[test]
fn mut_call_should_be_built_on_every_written_callee() {
    let nines = "interface Filler {\n    fn fill(self, mut xs: [i64])\n}\n\
        class Nines: Filler {\n    n: i64\n    fn fill(self, mut xs: [i64]) {\n        xs[0] = 9\n    }\n}\n";
    let rows = [
        (
            "function",
            format!("{SET}fn main() {{\n    let mut xs = [1]\n    set(mut xs)\n    print(xs)\n}}"),
        ),
        (
            "method",
            "class C {\n    n: i64\n    fn fill(self, mut xs: [i64]) {\n        xs[0] = 9\n    }\n}\n\
            fn main() {\n    let c = C(n: 1)\n    let mut xs = [1]\n    c.fill(mut xs)\n    print(xs)\n}"
                .to_owned(),
        ),
        (
            "generic function",
            "fn swap<T>(mut a: T, mut b: T) {\n    let t = a\n    a = b\n    b = t\n}\n\
            fn main() {\n    let mut x = 1\n    let mut y = 2\n    swap(mut x, mut y)\n    \
            print(x)\n    print(y)\n}"
                .to_owned(),
        ),
        (
            "`?.` call",
            "class C {\n    n: i64\n    fn fill(self, mut xs: [i64]) {\n        xs[0] = 9\n    }\n}\n\
            fn main() {\n    let c: C? = C(n: 1)\n    let mut xs = [1]\n    c?.fill(mut xs)\n    \
            print(xs)\n}"
                .to_owned(),
        ),
        (
            "bound method",
            format!(
                "{nines}fn run<T: Filler>(t: T, mut xs: [i64]) {{\n    t.fill(mut xs)\n}}\n\
                fn main() {{\n    let mut xs = [1]\n    run(Nines(n: 1), mut xs)\n    print(xs)\n}}"
            ),
        ),
        (
            "`any I` method",
            format!(
                "{nines}fn main() {{\n    let f: any Filler = Nines(n: 1)\n    let mut xs = [1]\n    \
                f.fill(mut xs)\n    print(xs)\n}}"
            ),
        ),
    ];
    for (row, src) in rows {
        let (program, ds) = checked(&src);
        assert!(ds.is_empty(), "row `{row}`: {ds:?}");
        let printed = program.expect("no errors").to_string();
        assert!(printed.contains("mut_call"), "row `{row}`: {printed}");
    }
}

/// GIVEN `let mut xs = [1]` whose only change is `set(mut xs)`, and a later `print(xs)`
/// WHEN it is compiled with the warnings of unused names
/// THEN there is no W0002 on `xs` and no W0014 on `set`'s parameter `xs`
#[test]
fn local_passed_mut_should_not_warn() {
    judge_unused();
    let src =
        format!("{SET}fn main() {{\n    let mut xs = [1]\n    set(mut xs)\n    print(xs)\n}}");
    codes(&src, &[]);
}

/// GIVEN `let mut xs = [1]`, `set(mut xs)` and no later read
/// WHEN it is compiled with the warnings of unused names
/// THEN W0014 names `xs`: passing it `mut` is a write
#[test]
fn let_mut_passed_mut_and_never_read_should_warn() {
    judge_unused();
    let src = format!("{SET}fn main() {{\n    let mut xs = [1]\n    set(mut xs)\n}}");
    codes(&src, &[(Code::UnusedVariable, (5, 13))]);
}

/// GIVEN `fn take(mut n: i64) -> i64` that returns the value it was passed, and a local or an
///   element that is passed `mut` to it with no other read
/// WHEN it is compiled with the warnings of unused names
/// THEN there is no W0014: the copy-in reads the place, and the result carries the value out
#[test]
fn place_passed_mut_to_a_call_with_a_result_should_count_as_read() {
    judge_unused();
    let take = "fn take(mut n: i64) -> i64 {\n    let old = n\n    n = 1\n    old\n}\n";
    let rows = [
        ("local", "let mut x = 2\n    print(take(mut x))"),
        ("element", "let mut xs = [2]\n    print(take(mut xs[0]))"),
        (
            "local in a loop",
            "let mut x = 2\n    for i in 0..2 {\n        print(take(mut x) + i)\n    }",
        ),
    ];
    for (row, body) in rows {
        let src = format!("{take}fn main() {{\n    {body}\n}}");
        codes_row(row, &src, &[]);
    }
    with_prelude();
    let src = "fn main() {\n    let mut x = 2\n    print(exchange(mut x, 3))\n}";
    codes_row("exchange", src, &[]);
}

/// GIVEN a local that is passed `mut` to `take`, a function with a result, and a local that is
///   only written
/// WHEN it is compiled with the warnings of unused names
/// THEN W0014 still names the local that is only written
#[test]
fn local_that_is_only_written_should_warn_beside_a_mut_call_with_a_result() {
    judge_unused();
    let src = "fn take(mut n: i64) -> i64 {\n    let old = n\n    n = 1\n    old\n}\n\
        fn main() {\n    let mut x = 2\n    print(take(mut x))\n    let mut y = 2\n    y = 3\n}";
    codes(src, &[(Code::UnusedVariable, (9, 13))]);
}

/// GIVEN `class Named { mut name: string }` and `fn rename(mut b: Named, name: string)` that only
///   writes `b.name`
/// WHEN it is checked
/// THEN W0002 warns on `b`, with no fix
#[test]
fn mut_class_parameter_never_reassigned_should_warn_with_the_class_help() {
    let src = "class Named {\n    mut name: string\n}\n\
        fn rename(mut b: Named, name: string) {\n    b.name = name\n}\n\
        fn main() {\n    let mut n = Named(name: \"a\")\n    rename(mut n, \"b\")\n    print(n.name)\n}";
    let d = diagnostic(src, Code::NeverReassigned, (4, 15, "b"), &[]);
    assert!(d.help().is_some(), "the class help");
    assert_eq!(d.fixes().len(), 0, "removing `mut` changes every caller");
}

/// GIVEN a `mut` parameter of a type that is not a class, which the function never reassigns
/// WHEN it is checked
/// THEN W0002 warns on the parameter, with no fix
#[test]
fn mut_array_parameter_never_reassigned_should_warn_without_help() {
    let src = "fn show(mut xs: [i64]) {\n    print(xs)\n}\n\
        fn main() {\n    let mut xs = [1]\n    show(mut xs)\n}";
    let d = diagnostic(src, Code::NeverReassigned, (1, 13, "xs"), &[]);
    assert_eq!(d.help(), None);
    assert_eq!(d.fixes().len(), 0);
}

/// GIVEN a `mut` parameter that holds class objects, as an optional, an array or an array of
///   arrays, which the function only writes through
/// WHEN it is checked
/// THEN W0002 warns on it with the class help: a write to a field is no reassignment
#[test]
fn mut_parameter_of_class_objects_should_warn_with_the_class_help() {
    let rows = [
        ("optional", "Named?", "b?.name = \"x\""),
        ("array", "[Named]", "b[0].name = \"x\""),
        ("array of arrays", "[[Named]]", "b[0][0].name = \"x\""),
    ];
    for (row, ty, stmt) in rows {
        let src = format!(
            "class Named {{\n    mut name: string\n}}\nfn f(mut b: {ty}) {{\n    {stmt}\n}}\nfn main() {{}}"
        );
        let d = diagnostic_row(row, &src, Code::NeverReassigned, (4, 10, "b"), &[]);
        assert!(d.help().is_some(), "row `{row}`: the class help");
    }
}

/// GIVEN a `mut` parameter of a type with no class object in it
/// WHEN it is checked
/// THEN W0002 warns with no help
#[test]
fn mut_parameter_without_class_objects_should_warn_without_help() {
    for ty in ["[i64]", "i64?", "[[string]]"] {
        let src = format!("fn f(mut b: {ty}) {{\n    print(b == b)\n}}\nfn main() {{}}");
        let found = diags(&src);
        let d = (found.iter().find(|d| d.code() == Code::NeverReassigned)).expect("a W0002");
        assert_eq!(d.help(), None, "type `{ty}`");
    }
}

/// GIVEN a call with a `mut` argument that is a `mut` parameter, in the first place or a later
///   one, that fails to resolve (an unknown function, an unknown method, a wrong count, an unknown
///   generic function, a field called as a method, an unknown constructor) or resolves to a
///   parameter that is not `mut`
/// WHEN it is checked
/// THEN the call's error is the only diagnostic: a `mut` argument counts as a write whatever the
///   call then gets wrong
#[test]
fn call_with_a_mut_argument_that_fails_should_give_its_error_only() {
    let decls = "fn set(mut n: i64) {\n    n += 1\n}\n\
        fn id(n: i64) -> i64 {\n    n\n}\n\
        class C {\n    x: i64\n    fn m(self, mut n: i64) {\n        n += 1\n    }\n}\n";
    let rows = [
        ("function, first", "ste(mut n)"),
        ("parameter not mut", "let _ = id(mut n)"),
        ("function, later", "ste(1, mut n)"),
        ("function, two", "ste(mut n, mut n)"),
        ("method, first", "c.ste(mut n)"),
        ("method, later", "c.ste(1, mut n)"),
        ("count, function", "set(mut n, 1)"),
        ("count, method", "c.m(mut n, mut n)"),
        ("generic", "ste<i64>(mut n)"),
        ("field", "c.x(mut n)"),
        ("constructor", "Foo(x: 1, mut n)"),
        ("element", "ste(mut xs[0])"),
        (
            "let mut local",
            "let mut k = 1\n    ste(mut k)\n    print(k)",
        ),
    ];
    for (row, stmt) in rows {
        let params = match row {
            "element" => "mut xs: [i64], c: C",
            "let mut local" => "c: C",
            _ => "mut n: i64, c: C",
        };
        let src = format!("{decls}fn _f({params}) {{\n    {stmt}\n}}\nfn main() {{}}");
        let found = diags(&src);
        let found: Vec<_> = found.iter().map(Diagnostic::severity).collect();
        assert_eq!(found, [Severity::Error], "row `{row}`: {src}");
    }
}

/// GIVEN a `mut` parameter in each kind of function, which the function never reassigns
/// WHEN it is checked
/// THEN W0002 warns on the parameter, and on no other
#[test]
fn mut_parameter_never_reassigned_should_warn_in_each_kind_of_function() {
    let rows = [
        (
            "function",
            "fn f(mut xs: [i64]) {\n    print(xs)\n}",
            (1, 10),
        ),
        (
            "generic function",
            "fn f<T>(mut xs: [T]) -> [T] {\n    xs\n}",
            (1, 13),
        ),
        (
            "method",
            "class C {\n    n: i64\n    fn f(self, mut xs: [i64]) {\n        print(xs)\n    }\n}",
            (3, 20),
        ),
        (
            "method of a generic class",
            "class C<T> {\n    n: T\n    fn f(self, mut xs: [T]) -> [T] {\n        xs\n    }\n}",
            (3, 20),
        ),
        (
            "method of an extend block",
            "class C {\n    n: i64\n}\nextend C {\n    fn f(self, mut xs: [i64]) {\n        print(xs)\n    }\n}",
            (5, 20),
        ),
        (
            "second parameter",
            "fn f(mut a: [i64], mut b: [i64]) {\n    a[0] = 1\n    print(b)\n}",
            (1, 24),
        ),
    ];
    for (row, decl, pos) in rows {
        let src = format!("{decl}\nfn main() {{}}");
        codes_row(row, &src, &[(Code::NeverReassigned, pos)]);
    }
}

/// GIVEN a `mut` parameter that the function reassigns in some way
/// WHEN it is checked
/// THEN there is no W0002: whole, element, `push`, or passed `mut` to another call, also through a
///   nested call
#[test]
fn mut_parameter_that_changes_should_not_warn() {
    let rows = [
        ("assigned", "fn f(mut xs: [i64]) {\n    xs = [1]\n}"),
        ("element", "fn f(mut xs: [i64]) {\n    xs[0] = 1\n}"),
        ("push", "fn f(mut xs: [i64]) {\n    xs.push(1)\n}"),
        ("passed mut", "fn f(mut xs: [i64]) {\n    set(mut xs)\n}"),
        (
            "passed mut to an element",
            "fn f(mut xs: [[i64]]) {\n    set(mut xs[0])\n}",
        ),
        (
            "method",
            "class C {\n    n: i64\n    fn f(self, mut xs: [i64]) {\n        set(mut xs)\n    }\n}",
        ),
        ("number", "fn f(mut n: i64) {\n    n += 1\n}"),
    ];
    for (row, decl) in rows {
        let src = format!("{SET}{decl}\nfn main() {{}}");
        codes_row(row, &src, &[]);
    }
}

/// GIVEN a function with a `todo` body and a `mut` parameter
/// WHEN it is checked
/// THEN there is no W0002 on the parameter: the body that is not yet written may change it
#[test]
fn mut_parameter_of_a_todo_body_should_not_warn() {
    codes(
        "fn f(mut xs: [i64]) {\n    todo\n}\nfn main() {}",
        &[(Code::TodoLeft, (2, 5))],
    );
}

/// GIVEN an interface requirement that spells `mut`, and a conformance method that never
///   reassigns the parameter, in the body of a class, in an `extend` block, for a generic
///   interface, for an inherited requirement and for a method that two interfaces require
/// WHEN it is checked
/// THEN there is no W0002: the caller needs the `mut` through the interface
#[test]
fn requirement_mut_should_not_warn() {
    let rows = [
        (
            "class body",
            "interface Filler {\n    fn fill(self, mut xs: [i64])\n}\n\
            class C: Filler {\n    n: i64\n    fn fill(self, mut xs: [i64]) {\n        print(xs)\n    }\n}",
        ),
        (
            "extend block",
            "interface Filler {\n    fn fill(self, mut xs: [i64])\n}\n\
            class C {\n    n: i64\n}\n\
            extend C: Filler {\n    fn fill(self, mut xs: [i64]) {\n        print(xs)\n    }\n}",
        ),
        (
            "generic interface",
            "interface Filler<T> {\n    fn fill(self, mut xs: [T])\n}\n\
            class C: Filler<i64> {\n    n: i64\n    fn fill(self, mut xs: [i64]) {\n        print(xs)\n    }\n}",
        ),
        (
            "inherited interface",
            "interface A {\n    fn fill(self, mut xs: [i64])\n}\n\
            interface B: A {\n    fn other(self)\n}\n\
            class C: B {\n    n: i64\n    fn fill(self, mut xs: [i64]) {\n        print(xs)\n    }\n    \
            fn other(self) {}\n}",
        ),
        (
            "two interfaces",
            "interface A {\n    fn fill(self, mut xs: [i64])\n}\n\
            interface B {\n    fn fill(self, mut xs: [i64])\n}\n\
            class C: A, B {\n    n: i64\n    fn fill(self, mut xs: [i64]) {\n        print(xs)\n    }\n}",
        ),
    ];
    for (row, decl) in rows {
        let src = format!("{decl}\nfn main() {{}}");
        codes_row(row, &src, &[]);
    }
}

/// GIVEN a conformance method with a `mut` parameter that its requirement does not spell, because
///   the requirement has the parameter without `mut`, or has no such parameter, or the method
///   spells `mut` on a second parameter too
/// WHEN it is checked
/// THEN E0152 reports the method, and W0002 does not warn on the same `mut`
#[test]
fn stray_mut_on_a_conformance_method_should_give_e0152_only() {
    let rows = [
        (
            "parameter without mut",
            "interface Filler {\n    fn fill(self, xs: [i64])\n}\n\
            class C: Filler {\n    n: i64\n    fn fill(self, mut xs: [i64]) {\n        print(xs)\n    }\n}",
        ),
        (
            "extra parameter",
            "interface Filler {\n    fn fill(self)\n}\n\
            class C: Filler {\n    n: i64\n    fn fill(self, mut xs: [i64]) {\n        print(xs)\n    }\n}",
        ),
        (
            "second parameter",
            "interface Filler {\n    fn fill(self, mut xs: [i64], ys: [i64])\n}\n\
            class C: Filler {\n    n: i64\n    fn fill(self, mut xs: [i64], mut ys: [i64]) {\n        print(xs)\n        print(ys)\n    }\n}",
        ),
        (
            "extend block",
            "interface Filler {\n    fn fill(self, xs: [i64])\n}\n\
            class C {\n    n: i64\n}\n\
            extend C: Filler {\n    fn fill(self, mut xs: [i64]) {\n        print(xs)\n    }\n}",
        ),
        (
            "generic interface",
            "interface Filler<T> {\n    fn fill(self, xs: [T])\n}\n\
            class C: Filler<i64> {\n    n: i64\n    fn fill(self, mut xs: [i64]) {\n        print(xs)\n    }\n}",
        ),
        (
            "inherited interface",
            "interface A {\n    fn fill(self, xs: [i64])\n}\n\
            interface B: A {\n    fn other(self)\n}\n\
            class C: B {\n    n: i64\n    fn fill(self, mut xs: [i64]) {\n        print(xs)\n    }\n    \
            fn other(self) {}\n}",
        ),
    ];
    for (row, decl) in rows {
        let src = format!("{decl}\nfn main() {{}}");
        let found: Vec<_> = diags(&src).iter().map(Diagnostic::code).collect();
        assert_eq!(found, [Code::ConformanceBody], "row `{row}`");
    }
}

/// GIVEN a requirement and a method with a different number of parameters, one of them `mut`
/// WHEN it is checked
/// THEN E0152 reports the method with no note: the parameters do not correspond
#[test]
fn conformance_with_another_parameter_count_should_give_e0152_without_a_note() {
    let rows = [
        (
            "method lacks a mut parameter",
            "interface Filler {\n    fn fill(self, mut xs: [i64], n: i64)\n}\n\
            class C: Filler {\n    n: i64\n    fn fill(self, n: i64) {\n        print(n)\n    }\n}",
        ),
        (
            "requirement lacks a mut parameter",
            "interface Filler {\n    fn fill(self, a: i64, mut xs: [i64])\n}\n\
            class C: Filler {\n    n: i64\n    fn fill(self, mut a: i64) {\n        a = 1\n    }\n}",
        ),
    ];
    for (row, decl) in rows {
        let src = format!("{decl}\nfn main() {{}}");
        diagnostic_row(row, &src, Code::ConformanceBody, (6, 8, "fill"), &[]);
    }
}

/// GIVEN a requirement and a conformance that differ in the type of a parameter that both spell
///   `mut`
/// WHEN it is checked
/// THEN E0152 reports the method, and W0002 does not warn on the parameter: the requirement spells
///   its `mut`, and the error is about the type
#[test]
fn requirement_mut_should_exempt_a_method_that_fails_on_a_type() {
    let src = "interface Filler {\n    fn fill(self, mut xs: [i64])\n}\n\
        class C: Filler {\n    n: i64\n    fn fill(self, mut xs: [bool]) {\n        print(xs)\n    }\n}\n\
        fn main() {}";
    diagnostic(src, Code::ConformanceBody, (6, 8, "fill"), &[]);
}

/// GIVEN two parameters of one name, both `mut`, or one of them
/// WHEN it is checked
/// THEN E0039 is the only diagnostic: the repeated name is poisoned and can be assigned, so
///   neither parameter gets W0002
#[test]
fn repeated_mut_parameter_should_give_the_repeat_only() {
    let rows = [
        ("both", "fn f(mut a: i64, mut a: i64) {\n}"),
        ("first", "fn f(mut a: i64, a: i64) {\n}"),
        ("second", "fn f(a: i64, mut a: i64) {\n}"),
    ];
    for (row, decl) in rows {
        let src = format!("{decl}\nfn main() {{}}");
        let found: Vec<_> = diags(&src).iter().map(Diagnostic::code).collect();
        assert_eq!(found, [Code::AlreadyDeclared], "row `{row}`");
    }
}

/// GIVEN a requirement and a conformance that differ in the `mut` of a parameter, in either
///   direction, in a class body, in an `extend` block and for a generic interface
/// WHEN it is checked
/// THEN E0152 reports the method, with a note on the requirement's parameter
#[test]
fn conformance_mut_that_differs_should_give_e0152_with_a_note() {
    let rows = [
        (
            "requirement mut, class body",
            "interface Filler {\n    fn fill(self, mut xs: [i64])\n}\n\
            class C: Filler {\n    n: i64\n    fn fill(self, xs: [i64]) {\n        print(xs)\n    }\n}",
            (6, 8, "fill"),
            (2, 23),
        ),
        (
            "requirement not mut, class body",
            "interface Filler {\n    fn fill(self, xs: [i64])\n}\n\
            class C: Filler {\n    n: i64\n    fn fill(self, mut xs: [i64]) {\n        xs[0] = 1\n    }\n}",
            (6, 8, "fill"),
            (2, 19),
        ),
        (
            "requirement mut, extend block",
            "interface Filler {\n    fn fill(self, mut xs: [i64])\n}\n\
            class C {\n    n: i64\n}\n\
            extend C: Filler {\n    fn fill(self, xs: [i64]) {\n        print(xs)\n    }\n}",
            (8, 8, "fill"),
            (2, 23),
        ),
        (
            "second parameter, generic interface",
            "interface Filler<T> {\n    fn fill(self, n: i64, mut xs: [T])\n}\n\
            class C: Filler<i64> {\n    n: i64\n    fn fill(self, n: i64, xs: [i64]) {\n        print(xs)\n    }\n}",
            (6, 8, "fill"),
            (2, 31),
        ),
    ];
    for (row, src, pos, note) in rows {
        let src = format!("{src}\nfn main() {{}}");
        diagnostic_row(row, &src, Code::ConformanceBody, pos, &[note]);
    }
}

/// GIVEN a conformance that differs from its requirement in the `mut` of a parameter and in the
///   type of another
/// WHEN it is checked
/// THEN E0152 reports the method once, with the note on the `mut` parameter
#[test]
fn conformance_with_mut_and_type_differing_should_give_one_e0152() {
    let src = "interface Filler {\n    fn fill(self, mut xs: [i64], n: i64)\n}\n\
        class C: Filler {\n    n: i64\n    fn fill(self, xs: [i64], n: bool) {\n        print(xs)\n    }\n}\n\
        fn main() {}";
    diagnostic(src, Code::ConformanceBody, (6, 8, "fill"), &[(2, 23)]);
}

/// The `MUT_KEY_STEPS` that checking `src` takes.
///
/// A path of 150 indexes nests deeper than a test's stack allows, so this runs on the compiler's.
fn key_steps(src: &str) -> usize {
    crate::on_compiler_stack(|| {
        MUT_KEY_STEPS.set(0);
        assert!(checked(src).0.is_some(), "{src}");
        MUT_KEY_STEPS.get()
    })
}

/// The program that calls `f` with `n` `mut` arguments: `mut xs<k>[0][0]`, each of a distinct
/// local.
fn many_arguments(n: usize) -> String {
    let params: Vec<String> = (0..n).map(|k| format!("mut a{k}: i64")).collect();
    let locals: Vec<String> = (0..n)
        .map(|k| format!("    let mut xs{k} = [[1]]\n"))
        .collect();
    let locals = locals.concat();
    let args: Vec<String> = (0..n).map(|k| format!("mut xs{k}[0][0]")).collect();
    format!(
        "fn f({}) {{}}\nfn main() {{\n{locals}    f({})\n}}",
        params.join(", "),
        args.join(", ")
    )
}

/// The program that calls `f` with one `mut` argument whose path has `len` indexes.
fn long_path(len: usize) -> String {
    let nest = (0..len).fold("i64".to_owned(), |ty, _| format!("[{ty}]"));
    let path = "[0]".repeat(len);
    let value = (0..len).fold("1".to_owned(), |v, _| format!("[{v}]"));
    format!(
        "fn f(mut a: i64) {{}}\nfn main() {{\n    let mut xs: {nest} = {value}\n    f(mut xs{path})\n}}"
    )
}

/// The overlap check reads each step of a path once, so its work grows in step with the source:
/// with the number of `mut` arguments, and with the length of one path. Two mutations fail it:
/// the pairwise compare of each two keys, and a prefix map keyed by the whole `Vec<Step>`.
#[test]
fn mut_place_overlap_should_be_linear() {
    let args = |n: usize| key_steps(&many_arguments(n));
    assert_near_linear(args(50), args(100), args(150));
    let path = |len: usize| key_steps(&long_path(len));
    assert_near_linear(path(50), path(100), path(150));
}

/// The number of expressions in `main` after checking `src`.
///
/// The path is deep, so this runs on the compiler's stack.
fn main_nodes(src: &str) -> usize {
    crate::on_compiler_stack(|| {
        let (program, _) = checked(src);
        let program = program.expect("the program should check");
        let main = program
            .functions
            .iter()
            .find(|f| f.name == "main")
            .expect("the program should have `main`");
        let mut all = Vec::new();
        main.body.exprs(&mut all);
        all.len()
    })
}

/// GIVEN a call with one `mut` argument whose place has 50, 100 and 150 indexes
/// WHEN it is checked
/// THEN the expressions of `main` grow in step with the source: the guard of the store-back
///   reads each array of the path once, not a copy of the path for each index
#[test]
fn mut_store_back_should_be_linear_in_the_depth_of_the_place() {
    let nodes = |len: usize| main_nodes(&long_path(len));
    assert_near_linear(nodes(50), nodes(100), nodes(150));
}

/// The `Code::MutMarkerMissing` count of `src`, and the other codes in order.
fn marker_codes(src: &str, code: Code) -> (usize, Vec<Code>) {
    let found = found_codes(src);
    let count = found.iter().filter(|&&c| c == code).count();
    (count, found.into_iter().filter(|&c| c != code).collect())
}

/// GIVEN a `mut` parameter for each way a call reaches a written callee: a function, a method, a
///   generic function, a generic method, a `?.` method, a bound `T: I` method and an `any I`
///   method
/// WHEN a call passes a place at the `mut` parameter, with no `mut`
/// THEN it is exactly one E0166
#[test]
fn missing_mut_should_be_reported_on_every_written_call_path() {
    let nines = "interface Filler {\n    fn fill(self, mut xs: [i64])\n}\n\
        class Nines: Filler {\n    n: i64\n    fn fill(self, mut xs: [i64]) {\n        xs[0] = 9\n    }\n}\n";
    let c =
        "class C {\n    n: i64\n    fn fill(self, mut xs: [i64]) {\n        xs[0] = 9\n    }\n}\n";
    let rows = [
        (
            "function",
            format!("{SET}fn main() {{\n    let mut xs = [1]\n    set(xs)\n    print(xs)\n}}"),
        ),
        (
            "method",
            format!(
                "{c}fn main() {{\n    let c = C(n: 1)\n    let mut xs = [1]\n    c.fill(xs)\n    print(xs)\n}}"
            ),
        ),
        (
            "generic function",
            "fn put<T>(mut a: T, b: T) {\n    a = b\n}\n\
            fn main() {\n    let mut x = 1\n    put(x, 2)\n    print(x)\n}"
                .to_owned(),
        ),
        (
            "generic method",
            "class Stack<T> {\n    items: [T]\n    fn put(self, mut a: T, b: T) {\n        a = b\n    }\n}\n\
            fn main() {\n    let s = Stack(items: [1])\n    let mut x = 1\n    s.put(x, 2)\n    print(x)\n}"
                .to_owned(),
        ),
        (
            "`?.` method",
            format!(
                "{c}fn main() {{\n    let c: C? = C(n: 1)\n    let mut xs = [1]\n    c?.fill(xs)\n    print(xs)\n}}"
            ),
        ),
        (
            "bound method",
            format!(
                "{nines}fn run<T: Filler>(t: T, mut xs: [i64]) {{\n    t.fill(xs)\n}}\n\
                fn main() {{\n    let mut xs = [1]\n    run(Nines(n: 1), mut xs)\n    print(xs)\n}}"
            ),
        ),
        (
            "`any I` method",
            format!(
                "{nines}fn main() {{\n    let f: any Filler = Nines(n: 1)\n    let mut xs = [1]\n    \
                f.fill(xs)\n    print(xs)\n}}"
            ),
        ),
    ];
    for (row, src) in rows {
        assert_eq!(
            found_codes(&src),
            [Code::MutMarkerMissing],
            "row `{row}`: {src}"
        );
    }
}

/// GIVEN a generic callee whose `mut` parameter is `T`, and `let mut x: i64? = None`
/// WHEN a call passes `x` with no `mut`, and another argument is an `i64` that `T` must fit
/// THEN it is E0166 only: the place binds `T = i64?` as `mut x` would, so the `5` is no cascade.
///   The same for a later `mut` position, a generic method, a method with its own type
///   parameter, a bound method, a call with explicit type arguments, and a type argument that
///   differs from the place
#[test]
fn unmarked_generic_mut_argument_should_give_the_missing_marker_only() {
    let putter = "interface Putter {\n    fn put<U>(self, mut a: U, b: U)\n}\n\
        class P: Putter {\n    n: i64\n    fn put<U>(self, mut a: U, b: U) {\n        a = b\n    }\n}\n";
    let holder = "interface Hold<T> {\n    fn put(self, mut a: T, b: T)\n}\n\
        class H: Hold<i64?> {\n    n: i64\n    fn put(self, mut a: i64?, b: i64?) {\n        a = b\n    }\n}\n";
    let rows = [
        (
            "function",
            "fn replace_with<T>(mut x: T, v: T) {\n    x = v\n}\n\
            fn main() {\n    let mut x: i64? = None\n    replace_with(x, 5)\n    print(x ?? 0)\n}"
                .to_owned(),
        ),
        (
            "mut position after the value",
            "fn replace_with<T>(v: T, mut x: T) {\n    x = v\n}\n\
            fn main() {\n    let mut x: i64? = None\n    replace_with(5, x)\n    print(x ?? 0)\n}"
                .to_owned(),
        ),
        (
            "two values after it",
            "fn replace_with<T>(mut x: T, v: T, w: T) {\n    x = v\n    x = w\n}\n\
            fn main() {\n    let mut x: i64? = None\n    replace_with(x, 5, 6)\n    print(x ?? 0)\n}"
                .to_owned(),
        ),
        (
            "explicit type arguments",
            "fn replace_with<T>(mut x: T, v: T) {\n    x = v\n}\n\
            fn main() {\n    let mut x: i64? = None\n    replace_with<i64?>(x, 5)\n    print(x ?? 0)\n}"
                .to_owned(),
        ),
        (
            "explicit type arguments that differ from the place",
            "fn replace_with<T>(mut x: T, v: T) {\n    x = v\n}\n\
            fn main() {\n    let mut x: i64? = None\n    replace_with<i64>(x, 5)\n    print(x ?? 0)\n}"
                .to_owned(),
        ),
        (
            "generic method",
            "class Stack<T> {\n    items: [T]\n    fn put(self, mut a: T, b: T) {\n        a = b\n    }\n}\n\
            fn main() {\n    let s: Stack<i64?> = Stack(items: [])\n    let mut x: i64? = None\n    \
            s.put(x, 5)\n    print(x ?? 0)\n}"
                .to_owned(),
        ),
        (
            "method with its own type parameter",
            "class C {\n    n: i64\n    fn put<U>(self, mut a: U, b: U) {\n        a = b\n    }\n}\n\
            fn main() {\n    let c = C(n: 1)\n    let mut x: i64? = None\n    c.put(x, 5)\n    print(x ?? 0)\n}"
                .to_owned(),
        ),
        (
            "bound method of a generic interface",
            format!(
                "{holder}fn run<C: Hold<i64?>>(c: C) {{\n    let mut x: i64? = None\n    c.put(x, 5)\n    \
                print(x ?? 0)\n}}\nfn main() {{\n    run(H(n: 1))\n}}"
            ),
        ),
        (
            "`any I` method of a generic interface",
            format!(
                "{holder}fn main() {{\n    let f: any Hold<i64?> = H(n: 1)\n    let mut x: i64? = None\n    \
                f.put(x, 5)\n    print(x ?? 0)\n}}"
            ),
        ),
        (
            "bound method",
            format!(
                "{putter}fn run<T: Putter>(t: T) {{\n    let mut x: i64? = None\n    t.put(x, 5)\n    \
                print(x ?? 0)\n}}\nfn main() {{\n    run(P(n: 1))\n}}"
            ),
        ),
    ];
    for (row, src) in rows {
        assert_eq!(
            found_codes(&src),
            [Code::MutMarkerMissing],
            "row `{row}`: {src}"
        );
    }
}

/// GIVEN a callee with a `mut` parameter, and a call with an unmarked argument for each kind of
///   argument that is no place
/// WHEN it is checked
/// THEN it is one E0166 for the marker, and any error of the argument itself
#[test]
fn unmarked_mut_argument_that_is_no_place_should_still_need_its_marker() {
    let rows = [
        ("literal", "set([1])", vec![Code::MutMarkerMissing]),
        ("call", "set(mk())", vec![Code::MutMarkerMissing]),
        (
            "name that is not in scope",
            "set(nope)",
            vec![Code::BindingNotFound, Code::MutMarkerMissing],
        ),
        (
            "place that is not in scope",
            "set(nope[0..1])",
            vec![Code::BindingNotFound, Code::MutMarkerMissing],
        ),
    ];
    for (row, call, want) in rows {
        let src = format!("{SET}fn mk() -> [i64] {{\n    [1]\n}}\nfn main() {{\n    {call}\n}}");
        let mut found = diags(&src);
        found.sort_by_key(|d| d.span().start());
        let found: Vec<_> = found.iter().map(Diagnostic::code).collect();
        assert_eq!(found, want, "row `{row}`: {src}");
    }
}

/// GIVEN a `mut` argument for each way a call reaches a callee that has no `mut` parameter
/// WHEN it is checked
/// THEN it is exactly one E0167
#[test]
#[expect(clippy::too_many_lines, reason = "one table, a row for each call path")]
fn stray_mut_should_be_reported_on_every_call_path() {
    with_prelude();
    let filler = "interface Plain {\n    fn take(self, xs: [i64])\n}\n\
        class Impl: Plain {\n    n: i64\n    fn take(self, xs: [i64]) {\n        print(xs)\n    }\n}\n";
    let head = "let xs = [1]\n    let y = 2\n    let ch = 'a'\n    let m = \"m\"\n    ";
    let rows: [(&str, String, &[Code]); 24] = [
        ("print", format!("{head}print(mut xs)"), &[]),
        ("eprint", format!("{head}eprint(mut xs)"), &[]),
        ("dbg", format!("{head}dbg(mut xs)"), &[]),
        ("panic", format!("{head}panic(mut m)"), &[]),
        ("todo", format!("{head}todo(mut m)"), &[Code::TodoLeft]),
        (
            "conversion",
            format!("{head}let n = i64(mut ch)\n    print(n)"),
            &[],
        ),
        (
            "alias of a number",
            format!(
                "type Count = i64\nfn main() {{\n    {head}let n = Count(mut ch)\n    print(n)\n}}"
            ),
            &[],
        ),
        (
            "alias of a class",
            format!(
                "class Board {{\n    mut cells: [i64]\n}}\ntype Grid = Board\nfn main() {{\n    {head}let b = Grid(cells: mut xs)\n    print(b.cells)\n}}"
            ),
            &[],
        ),
        (
            "class",
            format!(
                "class Board {{\n    mut cells: [i64]\n}}\nfn main() {{\n    {head}let b = Board(cells: mut xs)\n    print(b.cells)\n}}"
            ),
            &[],
        ),
        (
            "generic class",
            format!(
                "class Holder<T> {{\n    item: T\n}}\nfn main() {{\n    {head}let h = Holder(item: mut y)\n    print(h.item)\n}}"
            ),
            &[],
        ),
        (
            "variant",
            format!(
                "enum Shape {{\n    Circle(i64)\n    Dot\n}}\nfn main() {{\n    {head}let s = Shape.Circle(mut y)\n    print(s)\n}}"
            ),
            &[],
        ),
        (
            "generic variant",
            format!(
                "enum Pair<T> {{\n    Two(T, T)\n    Nil\n}}\nfn main() {{\n    {head}let p = Pair.Two(mut y, 3)\n    print(p)\n}}"
            ),
            &[],
        ),
        (
            "Some",
            format!("{head}let s = Some(mut y)\n    print(s ?? 0)"),
            &[],
        ),
        (
            "Ok",
            format!("{head}let r: Result<i64, string> = Ok(mut y)\n    print(r)"),
            &[],
        ),
        (
            "Err",
            format!("{head}let r: Result<i64, string> = Err(mut m)\n    print(r)"),
            &[],
        ),
        (
            "push",
            format!("{head}let mut ys = [1]\n    ys.push(mut y)"),
            &[],
        ),
        (
            "string method",
            format!("{head}let g = m.get(mut 0..1)\n    print(g ?? \"\")"),
            &[],
        ),
        (
            "array get with a range",
            format!("{head}let g = xs.get(mut 0..1)"),
            &[Code::Mismatch],
        ),
        (
            "prelude number method",
            format!("{head}let n = y.max(mut y)\n    print(n)"),
            &[],
        ),
        (
            "number method",
            format!("{head}let f = 2.0\n    let g = f.pow(mut f)\n    print(g)"),
            &[],
        ),
        (
            "bound method",
            format!(
                "{filler}fn run<T: Plain>(t: T, xs: [i64]) {{\n    let ys = xs\n    t.take(mut ys)\n}}\nfn main() {{\n    run(Impl(n: 1), [1])\n}}"
            ),
            &[],
        ),
        (
            "`any I` method",
            format!(
                "{filler}fn main() {{\n    let f: any Plain = Impl(n: 1)\n    {head}f.take(mut xs)\n}}"
            ),
            &[],
        ),
        (
            "`?.` call",
            format!(
                "class C {{\n    n: i64\n    fn show(self, xs: [i64]) {{\n        print(xs)\n    }}\n}}\nfn main() {{\n    let c: C? = C(n: 1)\n    {head}c?.show(mut xs)\n}}"
            ),
            &[],
        ),
        (
            "written method",
            format!(
                "class C {{\n    n: i64\n    fn show(self, xs: [i64]) {{\n        print(xs)\n    }}\n}}\nfn main() {{\n    let c = C(n: 1)\n    {head}c.show(mut xs)\n}}"
            ),
            &[],
        ),
    ];
    for (row, body, extra) in rows {
        let src = if body.contains("fn main") {
            body
        } else {
            format!("fn main() {{\n    {body}\n}}")
        };
        let (count, others) = marker_codes(&src, Code::MutMarkerNotMut);
        assert_eq!(count, 1, "row `{row}`: {src}: {others:?}");
        assert_eq!(others, extra, "row `{row}`: {src}");
    }
}

/// GIVEN a label on a call of a function, and a `mut` in either place
/// WHEN it is checked
/// THEN the label is E0055 only: the argument is not checked for its marker
#[test]
fn labeled_mut_argument_of_a_function_should_give_the_label_error_only() {
    for call in [
        "set(mut n: xs)",
        "set(n: mut xs)",
        "plain(mut n: xs)",
        "plain(n: mut xs)",
    ] {
        let src = format!(
            "{SET}fn plain(xs: [i64]) {{\n    print(xs)\n}}\nfn main() {{\n    let xs = [1]\n    {call}\n}}"
        );
        assert_eq!(found_codes(&src), [Code::LabeledArgument], "`{call}`");
    }
}

/// The source of nested calls `depth` deep: each argument is a place whose index is the next
/// call, and none has its `mut`. `callee` is `f` or `g`.
fn nested_missing(callee: &str, depth: usize) -> String {
    let mut arg = "a[0]".to_owned();
    for _ in 0..depth {
        arg = format!("a[{callee}({arg})]");
    }
    format!(
        "fn f(mut x: i64) -> i64 {{\n    x = 0\n    0\n}}\n\
        fn g<T>(mut x: T) -> i64 {{\n    x = x\n    0\n}}\n\
        fn main() {{\n    let mut a = [0]\n    let n = {callee}({arg})\n    print(n)\n}}"
    )
}

/// An argument that is no `mut` argument is checked once to find its fix, so a missing marker
/// does not double the work at each level of nesting: the expressions checked grow in step with
/// the source. A fix that checked each argument again would do 2^12 checks at depth 12.
#[test]
fn missing_mut_fix_should_be_linear() {
    for callee in ["f", "g"] {
        let checks = |depth: usize| {
            let src = nested_missing(callee, depth);
            EXPR_CHECKS.set(0);
            let found = found_codes(&src);
            assert!(found.contains(&Code::MutMarkerMissing), "{src}");
            EXPR_CHECKS.get()
        };
        assert_near_linear(checks(4), checks(8), checks(12));
    }
}

/// GIVEN a generic callee whose `mut` parameter is `T`, and a value with no `mut` at it, for a
///   function, a method, a bound method, a call with explicit type arguments,
///   and a call nested in another generic call
/// WHEN it is checked
/// THEN it is E0166 only: the value binds `T` as any argument does, so `T` is no cascade
#[test]
fn unmarked_value_at_a_generic_mut_parameter_should_give_the_missing_marker_only() {
    let decls = "fn id<T>(x: T) -> T {\n    x\n}\nfn g<T>(mut x: T) -> T {\n    let y = x\n    x = y\n    y\n}\n\
        fn h<T>(a: T, b: T) -> T {\n    a\n}\n\
        interface Gt {\n    fn g<T>(self, mut x: T) -> T\n}\n\
        class C: Gt {\n    n: i64\n    fn g<T>(self, mut x: T) -> T {\n        let y = x\n        x = y\n        y\n    }\n}\n";
    let rows = [
        ("function", "let r = g(1)\n    print(r)"),
        (
            "nested in a call that fixes T",
            "let b: u8 = 1\n    print(h(g(2), b))",
        ),
        (
            "nested in an identity call",
            "let r = id(g(1))\n    print(r)",
        ),
        ("explicit type arguments", "let r = g<i64>(1)\n    print(r)"),
        (
            "method",
            "let c = C(n: 1)\n    let r = c.g(1)\n    print(r)",
        ),
        (
            "place nested in a call that fixes T",
            "let mut c: u8 = 2\n    let b: u8 = 1\n    print(h(g(c), b))",
        ),
    ];
    for (row, body) in rows {
        let src = format!("{decls}fn main() {{\n    {body}\n}}");
        let found = found_codes(&src);
        assert_eq!(found, [Code::MutMarkerMissing], "row `{row}`: {src}");
    }
    let bound = format!(
        "{decls}fn run<T: Gt>(t: T) {{\n    let r = t.g(1)\n    print(r)\n}}\nfn main() {{\n    run(C(n: 1))\n}}"
    );
    assert_eq!(
        found_codes(&bound),
        [Code::MutMarkerMissing],
        "bound method"
    );
}

/// GIVEN a `mut` argument that is a temporary whose type comes from its context: `[]`, `None`,
///   and arrays of them
/// WHEN the program has an unrelated error before the call, after it, or in another function
/// THEN the argument is still E0169: the unrelated error does not hide it
#[test]
fn context_literal_temporary_should_be_reported_as_no_place_beside_an_unrelated_error() {
    for value in ["[]", "None", "[[]]", "[None]", "[[], []]", "Some(None)"] {
        for (row, before, after, other) in [
            ("before", "print(nope)\n    ", "", ""),
            ("after", "", "\n    print(nope)", ""),
            (
                "in another function",
                "",
                "",
                "fn other() {\n    let z = nope\n}\n",
            ),
            ("alone", "", "", ""),
        ] {
            let src = format!("{SET}{other}fn main() {{\n    {before}set(mut {value}){after}\n}}");
            let found = found_codes(&src);
            assert!(
                found.contains(&Code::MutArgumentNotPlace),
                "`{value}` {row}: {found:?}"
            );
        }
    }
}

/// GIVEN a function name as an argument at a `mut` parameter, with and without `mut`
/// WHEN it is checked
/// THEN it is E0105 as for any value, and no assignment error; with no `mut` E0166 follows, as the
///   marker is needed whatever the argument is
#[test]
fn function_as_a_mut_argument_should_be_reported_as_a_value() {
    let decls = "fn bump(mut x: i64) {\n    x = 1\n}\nfn answer() -> i64 {\n    42\n}\n";
    let marked = format!("{decls}fn main() {{\n    bump(mut answer)\n}}");
    assert_eq!(found_codes(&marked), [Code::TypeAsValue]);
    let unmarked = format!("{decls}fn main() {{\n    bump(answer)\n}}");
    assert_eq!(
        found_codes(&unmarked),
        [Code::TypeAsValue, Code::MutMarkerMissing]
    );
    // A place whose root is a function is a value too.
    for place in ["answer[0]", "answer.n"] {
        let marked = format!("{decls}fn main() {{\n    bump(mut {place})\n}}");
        let found = found_codes(&marked);
        assert!(
            !found.contains(&Code::AssignToFunction),
            "`mut {place}`: {found:?}"
        );
        assert!(
            found.contains(&Code::TypeAsValue),
            "`mut {place}`: {found:?}"
        );
        let unmarked = format!("{decls}fn main() {{\n    bump({place})\n}}");
        let found = found_codes(&unmarked);
        assert!(
            !found.contains(&Code::AssignToFunction),
            "`{place}`: {found:?}"
        );
        assert!(found.contains(&Code::TypeAsValue), "`{place}`: {found:?}");
    }
}

/// GIVEN a constructor that takes no labels, and a label on its `mut` argument
/// WHEN it is checked
/// THEN the label is E0055 only: the argument is not checked for its marker
#[test]
fn labeled_mut_argument_of_a_constructor_without_labels_should_give_the_label_error_only() {
    let decls =
        "enum Shape {\n    Circle(i64)\n    Dot\n}\nenum Pair<T> {\n    Two(T, T)\n    Nil\n}\n";
    for call in [
        "Some(mut x: a)",
        "Some(x: mut a)",
        "Shape.Circle(mut r: a)",
        "Shape.Circle(r: mut a)",
        "Pair.Two(mut p: a, 3)",
    ] {
        let src =
            format!("{decls}fn main() {{\n    let a = 1\n    let v = {call}\n    print(v)\n}}");
        let found = found_codes(&src);
        assert!(
            !found.contains(&Code::MutMarkerNotMut),
            "`{call}`: {found:?}"
        );
        assert!(
            found.contains(&Code::LabeledArgument),
            "`{call}`: {found:?}"
        );
    }
}

/// GIVEN a call of a built-in with the wrong number of arguments, or of a method that does not
///   exist, and a `mut` argument
/// WHEN it is checked
/// THEN there is no E0167: no parameter exists for the argument, as for a written callee
#[test]
fn stray_mut_should_not_be_reported_beside_a_count_error() {
    with_prelude();
    let head =
        "let xs = [1]\n    let y = 2\n    let ch = 'a'\n    let m = \"m\"\n    let f = 2.0\n    ";
    for call in [
        "read_line(mut y)",
        "print(mut y, mut y)",
        "panic(mut m, mut m)",
        "i64(mut ch, mut ch)",
        "xs.push(mut y, mut y)",
        "xs.len(mut y)",
        "m.len(mut y)",
        "m.get(mut 0..1, mut 0..1)",
        "f.pow(mut f, mut f)",
        "y.to_string(mut y)",
        "xs.nosuch(mut y)",
        "m.nosuch(mut y)",
    ] {
        let src = format!("fn main() {{\n    {head}{call}\n}}");
        let found = found_codes(&src);
        assert!(!found.is_empty(), "`{call}`: no error");
        assert!(
            !found.contains(&Code::MutMarkerNotMut),
            "`{call}`: {found:?}"
        );
    }
}

/// GIVEN a call with an unmarked place at a `mut` parameter, and a result of a type that the
///   call's context does not expect: a typed `let`, a `return`, an argument of another call and a
///   field of a constructor, with a `let` local, a `let mut` local and a field, for a function
///   and a method, generic or not
/// WHEN it is checked
/// THEN E0166 is reported, and the argument that it rejects is a failed part (the call row of
///   S14). The result of a generic call is untyped and adds no mismatch. The closed `i64` of
///   `count` is typed, so its mismatch E0032 comes first
#[test]
fn unmarked_place_should_leave_a_mismatch_only_to_a_closed_result() {
    let decls = "fn first<T>(mut xs: [T]) -> T {\n    let x = xs[0]\n    xs[0] = x\n    x\n}\n\
        fn count(mut xs: [i64]) -> i64 {\n    xs[0] += 1\n    xs[0]\n}\n\
        fn text(s: string) {\n    print(s)\n}\n\
        class Holder {\n    s: string\n}\n\
        class Names {\n    cells: [string]\n    fn pick<T>(self, mut xs: [T]) -> T {\n        let x = xs[0]\n        xs[0] = x\n        x\n    }\n}\n";
    let main = |body: &str| format!("{decls}fn main() {{\n    {body}\n}}");
    let e166 = Code::MutMarkerMissing;
    error_rows(&[
        (
            "let",
            main("let names = [\"a\"]\n    let n: i64 = first(names)\n    print(n)"),
            vec![(e166, (26, 24))],
        ),
        (
            "let mut",
            main("let mut names = [\"a\"]\n    let n: i64 = first(names)\n    print(n)"),
            vec![(e166, (26, 24))],
        ),
        // The closed `i64` of `count` is typed, so its mismatch stays.
        (
            "function that is not generic",
            main("let ys = [1]\n    let s: string = count(ys)\n    print(s)"),
            vec![(Code::Mismatch, (26, 21)), (e166, (26, 27))],
        ),
        (
            "field",
            main("let b = Names(cells: [\"a\"])\n    let n: i64 = first(b.cells)\n    print(n)"),
            vec![(e166, (26, 24))],
        ),
        (
            "method",
            main(
                "let b = Names(cells: [\"a\"])\n    let ys = [\"a\"]\n    let n: i64 = b.pick(ys)\n    print(n)",
            ),
            vec![(e166, (27, 25))],
        ),
        (
            "argument of another call",
            main("let ys = [1]\n    text(first(ys))"),
            vec![(e166, (26, 16))],
        ),
        (
            "field of a constructor",
            main("let ys = [1]\n    let h = Holder(s: first(ys))\n    print(h.s)"),
            vec![(e166, (26, 29))],
        ),
        (
            "return",
            format!("{decls}fn give(ys: [i64]) -> string {{\n    first(ys)\n}}\nfn main() {{}}"),
            vec![(e166, (25, 11))],
        ),
    ]);
}

/// GIVEN two `mut` places, and `f(mut a, mut b)`
/// WHEN the places are the same, or only the index, the field or the object differs
/// THEN only the same place is E0171: a literal index, a field of a field and a field of an
///   element are each the same place when they are written the same
#[test]
fn mut_places_should_overlap_only_when_every_step_is_the_same() {
    let decls = "class Item {\n    mut n: i64\n}\nclass Outer {\n    mut inner: Item\n}\n\
        fn f(mut a: i64, mut b: i64) {\n    a = 1\n    b = 2\n}\n";
    let xs = "let mut xs = [1, 2]";
    let o = "let o = Outer(inner: Item(n: 1))";
    let items = "let items = [Item(n: 1), Item(n: 2)]";
    let rows = [
        ("two literal indexes", xs, "mut xs[0], mut xs[1]", false),
        ("the same literal index", xs, "mut xs[0], mut xs[0]", true),
        (
            "the same field of a field",
            o,
            "mut o.inner.n, mut o.inner.n",
            true,
        ),
        (
            "the same field of an element",
            items,
            "mut items[0].n, mut items[0].n",
            true,
        ),
        (
            "fields of two elements",
            items,
            "mut items[0].n, mut items[1].n",
            false,
        ),
    ];
    for (row, setup, args, twice) in rows {
        let src = format!("{decls}fn main() {{\n    {setup}\n    f({args})\n}}");
        let expected: &[Code] = if twice { &[Code::MutPlaceTwice] } else { &[] };
        assert_eq!(found_codes(&src), expected, "row `{row}`");
    }
}

/// The hash of `value` with the standard hasher.
fn hash_of(value: &impl std::hash::Hash) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::hash::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// GIVEN the steps and the roots of a `mut` place
/// WHEN they are compared and hashed
/// THEN each equals itself, including a `Temp` root and an `Unknown` step, and no two different
///   steps are equal or hash alike: the lookups of `twice` stay constant
#[test]
fn mut_place_keys_should_equal_themselves_only() {
    let steps = [
        Step::Field(0),
        Step::Field(1),
        Step::Literal(0),
        Step::Literal(1),
        Step::Local(LocalId(0)),
        Step::Local(LocalId(1)),
        Step::Unknown,
    ];
    for (i, a) in steps.iter().enumerate() {
        for (j, b) in steps.iter().enumerate() {
            assert_eq!(a == b, i == j, "{a:?} and {b:?}");
            assert_eq!(hash_of(a) == hash_of(b), i == j, "{a:?} and {b:?}");
        }
    }
    let roots = [
        RootKey::Local(LocalId(0)),
        RootKey::Local(LocalId(1)),
        RootKey::Temp,
    ];
    for (i, a) in roots.iter().enumerate() {
        for (j, b) in roots.iter().enumerate() {
            assert_eq!(a == b, i == j, "{a:?} and {b:?}");
        }
    }
}

/// GIVEN `fn set(mut xs: [i64])`
/// WHEN a call passes a value with no `mut`: a string or an `i64` for the array
/// THEN the value is checked as any other: the mismatch is E0032, and E0166 follows
#[test]
fn unmarked_value_at_a_mut_parameter_should_be_checked_as_a_value() {
    for (row, call) in [
        ("string for an array", "set(\"x\")"),
        ("i64 for an array", "set(5)"),
    ] {
        let src = format!("{SET}fn main() {{\n    {call}\n}}");
        let mut found = diags(&src);
        found.sort_by_key(|d| d.span().start());
        let found: Vec<_> = found.iter().map(Diagnostic::code).collect();
        assert_eq!(
            found,
            [Code::Mismatch, Code::MutMarkerMissing],
            "row `{row}`"
        );
    }
}

/// The E0166 diagnostics of `src`, in the order of their places, each as the line and the column
/// of its place and whether it offers a fix.
fn marker_fixes(src: &str) -> Vec<(usize, usize, bool)> {
    let mut ds = diags(src);
    ds.sort_by_key(|d| d.span().start());
    ds.iter()
        .map(|d| {
            assert_eq!(d.code(), Code::MutMarkerMissing, "{src}");
            let (line, col) = at(src, d.span());
            (line, col, !d.fixes().is_empty())
        })
        .collect()
}

/// GIVEN `f(mut xs[0], xs[1], xs, n)`, where `xs[0]` and `xs[1]` are inside `xs`, and `n` is a
///   plain local
/// WHEN the unmarked arguments are reported
/// THEN `xs[1]` and `xs` get no fix, as adding `mut` to each would pass overlapping places, and
///   `n` gets the fix. `xs[1]` meets no place before it: only the keys read from the other end
///   find the pair, and that pair does not start at the last key.
#[test]
fn unmarked_place_inside_a_later_place_should_get_no_fix() {
    let src = "fn f(mut a: i64, mut b: i64, mut c: [i64], mut d: i64) {\n    a = 1\n    b = 2\n    \
        c[0] = 3\n    d = 4\n}\n\
        fn main() {\n    let mut xs = [1, 2]\n    let mut n = 0\n    f(mut xs[0], xs[1], xs, n)\n    \
        print(xs)\n    print(n)\n}";
    assert_eq!(
        marker_fixes(src),
        [(10, 18, false), (10, 25, false), (10, 29, true)]
    );
    // With no place after the pair, `xs[1]` is the key just before the last.
    let short = "fn f(mut a: i64, mut b: i64, mut c: [i64]) {\n    a = 1\n    b = 2\n    c[0] = 3\n}\n\
        fn main() {\n    let mut xs = [1, 2]\n    f(mut xs[0], xs[1], xs)\n    print(xs)\n}";
    assert_eq!(marker_fixes(short), [(8, 18, false), (8, 25, false)]);
    // A place that meets nothing keeps its fix, between the pair.
    let between = "fn f(mut a: i64, mut b: [i64], mut c: [i64]) {\n    a = 1\n    b[0] = 2\n    c[0] = 3\n}\n\
        fn main() {\n    let mut xs = [1, 2]\n    let mut ys = [3]\n    f(mut xs[0], ys, xs)\n    \
        print(xs)\n    print(ys)\n}";
    assert_eq!(marker_fixes(between), [(9, 18, true), (9, 22, false)]);
}

/// GIVEN a generic function `first<T>(mut xs: [T]) -> T`, and `bump(mut first(xs))`, where the
///   call is the argument of a `mut` parameter and so is checked with no type expected of it
/// WHEN it is checked
/// THEN `first(xs)` is E0169, as a call is no place, and E0166 has the fix: the result meets a
///   context that expects nothing
#[test]
fn unmarked_place_whose_result_has_no_context_should_get_its_fix() {
    let src = "fn first<T>(mut xs: [T]) -> T {\n    let x = xs[0]\n    xs[0] = x\n    x\n}\n\
        fn bump(mut n: i64) {\n    n += 1\n}\n\
        fn main() {\n    let mut xs = [1]\n    bump(mut first(xs))\n    print(xs)\n}";
    let mut ds = diags(src);
    ds.sort_by_key(|d| d.span().start());
    let found: Vec<_> = ds.iter().map(|d| (d.code(), at(src, d.span()))).collect();
    assert_eq!(
        found,
        [
            (Code::MutArgumentNotPlace, (11, 14)),
            (Code::MutMarkerMissing, (11, 20))
        ]
    );
    assert!(!ds[1].fixes().is_empty(), "E0166 offers its fix");
}

/// GIVEN a stray `mut` that a line break separates from its argument
/// WHEN it is E0167
/// THEN the fix removes `mut` alone, and the line break stays
#[test]
fn stray_mut_before_a_line_break_should_be_removed_alone() {
    let src = "fn main() {\n    let mut xs = [1, 2]\n    print(mut\n        xs)\n    xs[0] = 3\n}";
    let d = only(src, Code::MutMarkerNotMut);
    assert_eq!(
        fixes(src, &d),
        [(true, vec![(3, 11, "mut", String::new())])]
    );
    let spaced = "fn main() {\n    let mut xs = [1, 2]\n    print(mut \t xs)\n    xs[0] = 3\n}";
    let d = only(spaced, Code::MutMarkerNotMut);
    assert_eq!(
        fixes(spaced, &d),
        [(true, vec![(3, 11, "mut \t ", String::new())])]
    );
}

/// GIVEN a generic `clear<T>(mut n: T?, d: T) -> T` and a call with an `i64` place
/// WHEN the call is checked and its result is used as a `string`
/// THEN it is E0170 only: the failed `mut` argument fails the call, so the result raises no more
///   errors
#[test]
fn generic_call_with_a_failed_mut_argument_should_not_cascade() {
    let src = "fn clear<T>(mut n: T?, d: T) -> T {\n    n = None\n    d\n}\n\
        fn text(s: string) {\n    print(s)\n}\n\
        fn main() {\n    let mut n = 1\n    let r = clear(mut n, 5)\n    text(r)\n}";
    assert_eq!(found_codes(src), [Code::MutArgumentType]);
}

/// GIVEN `f(mut xs[i], 5, k, 7 + 1, xs[j])`, where `i` is a `let mut` local and `k` and `j` are
///   `let` locals
/// WHEN the `tir` is printed
/// THEN a literal and a `let` local stay in the call, as nothing can change them; the `let mut`
///   index, the sum and the element are each read into a hidden local first, in written order
#[test]
fn mut_call_should_hide_each_argument_that_a_mut_argument_can_change() {
    let src = "fn f(mut a: i64, b: i64, c: i64, d: i64, e: i64) {\n    a = b + c + d + e\n}\n\
        fn main() {\n    let mut xs = [1, 2, 3]\n    let j = 1\n    let mut i = 0\n    let k = 2\n    \
        f(mut xs[i], 5, k, 7 + 1, xs[j])\n    print(xs)\n    i = 0\n}";
    contains(
        src,
        &[
            "(block (let #5 i#2) (let #6 (+ 7 1)) (let #7 (index xs#0 j#1)) (mut_call (call f #4 5 k#3 #6 #7)",
            "(mut #4 (index xs#0 #5))",
        ],
    );
}

/// GIVEN a generic `f` with a `mut` array parameter after an `i64`, and a generic `g` with the
///   same parameters but no `mut`, each nested as an argument of another generic call
/// WHEN the checker ranks the nested call
/// THEN the place at the `mut` parameter is not ranked, as a place has no rank to find: the call
///   of `f` costs one step less than the call of `g`, which ranks its array argument
#[test]
fn place_at_a_mut_parameter_should_not_be_ranked() {
    crate::on_compiler_stack(|| {
        let steps = |inner: &str| {
            let src = format!(
                "fn f<T>(n: i64, mut xs: [T]) -> T {{\n    let x = xs[0]\n    xs[0] = x\n    x\n}}\n\
                fn g<T>(n: i64, xs: [T]) -> T {{\n    xs[0]\n}}\n\
                fn k<T>(a: T, b: T) -> T {{\n    b\n}}\n\
                fn main() {{\n    let mut xs = [1]\n    let r: i64 = k(1, {inner})\n    print(r)\n    \
                xs[0] = 2\n}}"
            );
            generic::RANK_STEPS.set(0);
            clean(&src);
            generic::RANK_STEPS.get()
        };
        let (with_mut, without) = (steps("f(0, mut xs)"), steps("g(0, xs)"));
        assert!(with_mut > 0, "the counter should count the walk");
        assert_eq!(with_mut + 1, without);
    });
}

/// GIVEN `c: C?` and a `mut` argument that is a field of `c`, an element of that field, a field of
///   an optional `b` of an object `a`, a field passed to a method, or the field of `exchange`
/// WHEN each program is checked
/// THEN each reports one E0062 with no fix, because `?.` makes the argument no place (E0170)
/// AND the read `print(c.xs)` keeps its `?.` fix
#[test]
fn optional_object_of_a_mut_argument_should_get_no_fix() {
    with_prelude();
    let head = "class C {\n    mut xs: [i64]\n    mut n: i64\n}\n\
        class A {\n    mut b: C?\n}\n\
        class O {\n    fn m(self, mut xs: [i64]) {\n        xs[0] = 1\n    }\n}\n\
        fn set(mut xs: [i64]) {\n    xs[0] = 1\n}\n\
        fn put(mut n: i64) {\n    n = 1\n}\n";
    for call in [
        "set(mut c.xs)",
        "put(mut c.xs[0])",
        "set(mut a.b.xs)",
        "O().m(mut c.xs)",
        "exchange(mut c.n, 1)",
    ] {
        let src = format!(
            "{head}fn main() {{\n    let c: C? = C(xs: [0], n: 0)\n    \
            let a = A(b: c)\n    {call}\n}}"
        );
        let (_, ds) = checked(&src);
        let errors: Vec<_> = ds.iter().filter(|d| d.code() == Code::MayBeNone).collect();
        assert_eq!(errors.len(), 1, "{call}: {ds:?}");
        assert_eq!(errors[0].fixes().len(), 0, "{call}");
    }
    let read = format!("{head}fn main() {{\n    let c: C? = C(xs: [0], n: 0)\n    print(c.xs)\n}}");
    let d = only(&read, Code::MayBeNone);
    assert_eq!(d.fixes().len(), 1, "a read keeps the `?.` fix");
}

/// GIVEN a call with an unmarked place at a `mut` parameter, whose result meets a context that
///   expects an optional, an `any` type, an array or an optional of a hole (`T` of a generic
///   call that its other arguments do not fix yet), or an unknown type (`[]`)
/// WHEN the missing `mut` is reported
/// THEN E0166 has the fix, and the call gives the codes of its row besides: a context that is
///   unknown, or that holds a hole, expects nothing, and the other contexts take a type that the
///   result converts to, so `mut` does not change the result
#[test]
fn unmarked_place_in_an_optional_or_any_context_should_get_its_fix() {
    let get = "fn get<T>(mut x: T) -> T {\n    let old = x\n    x = old\n    old\n}\n";
    let named = "interface Named {\n    fn name(self) -> string\n}\n\
        class Cat: Named {\n    fn name(self) -> string {\n        \"cat\"\n    }\n}\n";
    let take = "fn take(mut xs: [i64]) -> [i64] {\n    xs[0] = 1\n    xs\n}\n";
    // A row: its name, the declarations, the body of `main`, the place of E0166, and the codes
    // that the program gives besides E0166.
    let rows = [
        (
            "optional",
            get.to_string(),
            "let mut ys = [1]\n    let o: [i64]? = get(ys)\n    ys[0] = 2\n    print(o == None)",
            (8, 25),
            vec![],
        ),
        (
            "any",
            format!("{named}{get}"),
            "let mut c = Cat()\n    let n: any Named = get(c)\n    print(n.name())",
            (16, 28),
            vec![],
        ),
        (
            "optional any",
            format!("{named}{get}"),
            "let mut c = Cat()\n    let n: (any Named)? = get(c)\n    print(n == None)",
            (16, 31),
            vec![],
        ),
        (
            "empty array",
            take.to_string(),
            "let mut ys = [1]\n    print([] == take(ys))",
            (7, 22),
            vec![],
        ),
        (
            "assigned to an array of a hole",
            take.to_string(),
            "let mut zs = []\n    let mut ys = [1]\n    zs = take(ys)\n    print(zs)",
            (8, 15),
            vec![Code::CannotInfer],
        ),
        (
            "coalesced with an empty array",
            take.to_string(),
            "let mut ys = [1]\n    let a = [] ?? take(ys)\n    print(a)",
            (7, 24),
            vec![Code::CannotInfer],
        ),
        (
            "argument of a generic call: an array of a hole",
            format!("{get}fn one<T>(a: [T]) -> T {{\n    a[0]\n}}\n"),
            "let mut ys = [1]\n    print(one(get(ys)))",
            (11, 19),
            vec![],
        ),
        (
            "argument of a generic call: an optional of a hole",
            format!("{get}fn first<T>(a: T?) -> T {{\n    a ?? first(a)\n}}\n"),
            "let mut o: i64? = 1\n    print(first(get(o)))",
            (11, 21),
            vec![],
        ),
    ];
    for (row, decls, body, (line, col), rest) in rows {
        let src = format!("{decls}fn main() {{\n    {body}\n}}");
        let mut ds = diags(&src);
        ds.sort_by_key(|d| d.span().start());
        let (missing, others): (Vec<_>, Vec<_>) =
            ds.iter().partition(|d| d.code() == Code::MutMarkerMissing);
        let others: Vec<_> = others.iter().map(|d| d.code()).collect();
        assert_eq!(others, rest, "row `{row}`");
        let missing: Vec<_> = (missing.iter())
            .map(|d| (at(&src, d.span()), !d.fixes().is_empty()))
            .collect();
        assert_eq!(missing, [((line, col), true)], "row `{row}`");
    }
}

/// GIVEN `class Box { mut x: [i64] }`, `fn f(mut x: [i64])` and `fn g(mut b: Box, mut x: [i64])`
/// WHEN `main` calls `f(mut b.x)`, and `g(mut b, mut b.x)`, and the `tir` is printed
/// THEN the lone call keeps no object, because no store-back of it can rebind `b`
/// AND the second call keeps the object of `b`, because the store-back of `mut b` can
#[test]
fn mut_call_should_keep_a_root_binding_only_when_an_argument_rebinds_it() {
    let decls = "class Box {\n    mut x: [i64]\n}\n\
        fn f(mut x: [i64]) {\n    x[0] = 1\n}\n\
        fn g(mut b: Box, mut x: [i64]) {\n    b = Box(x: [2])\n    x[0] = 3\n}\n";
    let lone = format!(
        "{decls}fn main() {{\n    let mut b = Box(x: [0])\n    f(mut b.x)\n    b = Box(x: [1])\n}}"
    );
    let printed = lowered(&lone);
    assert!(printed.contains("(mut_call (call f"), "{printed}");
    assert!(!printed.contains("(keep "), "{printed}");
    let pair =
        format!("{decls}fn main() {{\n    let mut b = Box(x: [0])\n    g(mut b, mut b.x)\n}}");
    let printed = lowered(&pair);
    assert_eq!(printed.matches("(keep ").count(), 1, "{printed}");
}

/// The errors of `src` in span order, each as its code and the source text of its place.
/// Warnings are left out, and two errors on one place stay in the order they are reported.
fn error_texts(src: &str) -> Vec<(Code, &str)> {
    let mut ds = diags(src);
    ds.retain(|d| d.code().severity() == Severity::Error);
    ds.sort_by_key(|d| d.span().start());
    let place = |d: &Diagnostic| &src[d.span().start()..d.span().end()];
    ds.iter().map(|d| (d.code(), place(d))).collect()
}

const E30: Code = Code::UnknownType;
const E32: Code = Code::Mismatch;
const E169: Code = Code::MutArgumentNotPlace;
const E170: Code = Code::MutArgumentType;

/// A row of a table of programs: its name, the items `extra` and the statements `body` for a
/// program builder, and the errors wanted, each as its code and the source text of its place.
type Row = (
    &'static str,
    &'static str,
    &'static str,
    &'static [(Code, &'static str)],
);

/// Checks each row of `rows` in the program that `program` builds from it: a failure names the row.
fn check_rows(rows: &[Row], program: fn(&str, &str) -> String) {
    for &(row, extra, body, wanted) in rows {
        let src = program(extra, body);
        assert_eq!(error_texts(&src), wanted, "row `{row}`");
    }
}

/// The callees that the rows of the `mut` argument tests with a written context call.
const CONTEXT_DECLS: &str = "fn first<T>(mut xs: [T]) -> T { xs = xs; xs[0] }\n\
    fn two<T>(mut a: [T], mut b: [T]) -> T { a = a; b = b; a[0] }\n\
    fn pair<T>(mut xs: [T], y: T) -> T { xs = xs; y }\n\
    fn pick<T>(n: i64, mut xs: [T]) -> T { xs = xs; xs[n] }\n\
    fn keep<T>(mut x: T?) -> T? { x = x; x }\n\
    fn need(n: i64) {}\n\
    class Bx { v: i64; fn get<T>(self, mut xs: [T]) -> T { xs = xs; xs[0] } }\n\
    class H { mut xs: [string] }\n";

/// A program with the callees of `CONTEXT_DECLS`, the items `extra`, and a `main` with some
/// places, then the statements `body`.
fn context_program(extra: &str, body: &str) -> String {
    format!(
        "{CONTEXT_DECLS}{extra}fn main() {{\n    let mut names = [\"a\"]\n    \
        let mut names2 = [\"b\"]\n    let mut ints = [1]\n    let mut mq: string? = None\n    \
        let b = Bx(v: 1)\n    let c = b.v > 0\n    let o: i64? = None\n    \
        let h = H(xs: [\"a\"])\n    {body}\n}}"
    )
}

/// GIVEN a generic callee with a `mut` parameter, and a `mut` argument whose type is not what
///   the written context makes of the parameter, in every place a context is written, with one
///   `mut` argument or two, with the other arguments fitting or not, and with a parameter that no
///   argument binds
/// WHEN the call is checked
/// THEN the `mut` argument is E0170 at its place and nothing else: no E0032 shares its place
///   AND the other argument that does not fit keeps its own E0032
#[test]
#[expect(clippy::too_many_lines, reason = "a table of rows, one for each form")]
fn mut_argument_against_a_written_context_should_be_e0170_only() {
    let rows: &[Row] = &[
        ("a", "", "let n: i64 = first(mut names)", &[(E170, "names")]),
        (
            "c: the other argument fits",
            "",
            "let n: i64 = pair(mut names, 1)",
            &[(E170, "names")],
        ),
        (
            "b: the other argument does not fit",
            "",
            "let n: i64 = pair(mut names, \"b\")",
            &[(E170, "names"), (E32, "\"b\"")],
        ),
        (
            "d: a generic method",
            "",
            "let n: i64 = b.get(mut names)",
            &[(E170, "names")],
        ),
        (
            "g: the `mut` parameter is second",
            "",
            "let n: i64 = pick(0, mut names)",
            &[(E170, "names")],
        ),
        (
            "f: an optional context",
            "",
            "let n: i64? = first(mut names)",
            &[(E170, "names")],
        ),
        (
            "h: an argument of a call",
            "",
            "need(first(mut names))",
            &[(E170, "names")],
        ),
        (
            "m: an element of an array",
            "",
            "let n: [i64] = [first(mut names)]",
            &[(E170, "names")],
        ),
        (
            "l: an optional parameter",
            "",
            "let n: i64? = keep(mut mq)",
            &[(E170, "mq")],
        ),
        (
            "e: the result of a function",
            "fn g(mut names: [string]) -> i64 { names = names; first(mut names) }\n",
            "print(g(mut names))",
            &[(E170, "names")],
        ),
        (
            "unbound: a parameter that no argument binds",
            "fn f<T, U>(mut xs: [T]) -> T { xs = xs; xs[0] }\n",
            "let n: i64 = f(mut names)",
            &[(E170, "names")],
        ),
        (
            "unbound-wrongb: the same, and another argument that does not fit",
            "fn f<T, U>(mut xs: [T], y: T) -> T { xs = xs; y }\n",
            "let n: i64 = f(mut names, \"b\")",
            &[(E170, "names"), (E32, "\"b\"")],
        ),
        (
            "two: two `mut` arguments",
            "",
            "let n: i64 = two(mut names, mut names2)",
            &[(E170, "names"), (E170, "names2")],
        ),
        (
            "ret: after `return`",
            "fn g(mut names: [string]) -> i64 { names = names; return first(mut names) }\n",
            "print(g(mut names))",
            &[(E170, "names")],
        ),
        (
            "brk: the value of `break`",
            "",
            "let n: i64 = loop { break first(mut names) }",
            &[(E170, "names")],
        ),
        (
            "coal: the right side of `??`",
            "",
            "let n: i64 = o ?? first(mut names)",
            &[(E170, "names")],
        ),
        (
            "ifb: a branch of `if`",
            "",
            "let n: i64 = if c { first(mut names) } else { 0 }",
            &[(E170, "names")],
        ),
        (
            "asg: an assignment",
            "",
            "let mut n: i64 = 0\n    n = first(mut names)",
            &[(E170, "names")],
        ),
        (
            "ctor: an argument of a constructor",
            "",
            "let b2 = Bx(v: first(mut names))",
            &[(E170, "names")],
        ),
        (
            "block: the value of a block",
            "",
            "let n: i64 = { first(mut names) }",
            &[(E170, "names")],
        ),
        (
            "match: an arm",
            "",
            "let n: i64 = match c {\n        true => first(mut names)\n        false => 0\n    }",
            &[(E170, "names")],
        ),
        (
            "nest: in the argument of another call",
            "",
            "let n: i64 = pair(mut ints, first(mut names))",
            &[(E170, "names")],
        ),
        (
            "field: a field is the place",
            "",
            "let n: i64 = first(mut h.xs)",
            &[(E170, "h.xs")],
        ),
        (
            "geni: a requirement of a generic parameter",
            "interface I { fn get<T>(self, mut xs: [T]) -> T }\n\
            class Cx: I { v: i64; fn get<T>(self, mut xs: [T]) -> T { xs = xs; xs[0] } }\n\
            fn use_g<U: I>(t: U, mut names: [string]) -> i64 { names = names; t.get(mut names) }\n",
            "print(use_g(Cx(v: 1), mut names))",
            &[(E170, "names")],
        ),
    ];
    check_rows(rows, context_program);
    // The E0170 sits on the argument itself, not on the call.
    for body in [
        "let n: i64 = first(mut names)",
        "let n: i64 = pair(mut names, \"b\")",
    ] {
        let src = context_program("", body);
        let want = src.find(body).expect("the body is in the program")
            + body.find("names").expect("the body names `names`");
        let ds = diags(&src);
        let starts: Vec<_> = (ds.iter())
            .filter(|d| d.code() == E170)
            .map(|d| d.span().start())
            .collect();
        assert_eq!(starts, [want], "`{body}`");
        assert!(
            !ds.iter()
                .any(|d| d.code() == E32 && d.span().start() == want),
            "`{body}`: no E0032 shares the place"
        );
    }
}

/// GIVEN a `mut` argument at a generic call with a written context, where the argument fits, or
///   only another argument does not fit, or the `mut` is missing or has a label, or the call
///   writes its type arguments, or is a safe call, or is not generic, or the place is behind `?.`,
///   or the parameter is an associated type, or a parameter that no argument binds is in context
/// WHEN the call is checked
/// THEN the reports are those of the call before, but a missing `mut` makes the call untyped
#[test]
fn mut_argument_against_a_written_context_should_keep_the_other_reports() {
    let rows: &[Row] = &[
        (
            "j: the context fits",
            "",
            "let s: string = first(mut names)",
            &[],
        ),
        (
            "k: only the other argument does not fit",
            "",
            "let n: string = pair(mut names, 1)",
            &[(E32, "1")],
        ),
        // E0166 rejects `names`, so the call has a failed part and is untyped (the call row of
        // S14): its mismatch waits.
        (
            "i: no `mut`",
            "",
            "let n: i64 = first(names)",
            &[(Code::MutMarkerMissing, "names")],
        ),
        (
            "n: a written type argument",
            "",
            "let n: i64 = first<string>(mut names)",
            &[(E32, "first<string>(mut names)")],
        ),
        (
            "twomix: only one of two does not fit",
            "",
            "let n: i64 = two(mut names, mut ints)",
            &[(E170, "ints")],
        ),
        (
            "lbl: a labeled `mut` argument",
            "",
            "let n: i64 = first(xs: mut names)",
            &[(Code::LabeledArgument, "xs"), (E32, "names")],
        ),
        (
            "safe: a safe call",
            "",
            "let bo: Bx? = Some(b)\n    let n: i64? = bo?.get(mut names)",
            &[(E170, "names")],
        ),
        (
            "nongen: a callee that is not generic",
            "fn fixed(mut xs: [string]) -> string { xs = xs; xs[0] }\n",
            "let n: i64 = fixed(mut names)",
            &[(E32, "fixed(mut names)")],
        ),
        (
            "chain: a place behind `?.`",
            "",
            "let ho: H? = Some(H(xs: [\"a\"]))\n    let n: i64 = first(mut ho?.xs)",
            &[(E170, "ho?.xs")],
        ),
        (
            "assoc: an associated type",
            "interface Src { type Item; fn get(self) -> Item }\n\
            class S: Src { v: i64; type Item = string; fn get(self) -> string { \"a\" } }\n\
            fn g<T: Src>(mut xs: [T.Item], s: T) -> T.Item { xs = xs; s.get() }\n",
            "let n: i64 = g(mut names, S(v: 1))",
            &[(E32, "g(mut names, S(v: 1))")],
        ),
        (
            "unbound-fit: an unbound parameter, and the context fits",
            "fn f<T, U>(mut xs: [T]) -> T { xs = xs; xs[0] }\n",
            "let n: string = f(mut names)",
            &[(Code::CannotInfer, "f")],
        ),
    ];
    check_rows(rows, context_program);
}

/// The callees and the values that the rows of the `mut` argument tests with a temporary use.
/// `bad` has a return type that fails (E0030), and so has no type.
const TEMPORARY_DECLS: &str = "fn set(mut n: i64) { n = 1 }\n\
    fn sets(mut s: string) { s = \"\" }\n\
    fn take<T>(mut x: T) { x = x }\n\
    fn id(mut n: i64) -> i64 { n = n; n }\n\
    fn bad() -> Nope { bad() }\n\
    interface J { fn put(self, mut n: i64) }\n\
    class O: J {\n    mut v: i64\n    fn set(self, mut n: i64) { n = 1 }\n    \
    fn put(self, mut n: i64) { n = 1 }\n}\n\
    fn num() -> i64 { 1 }\n";

/// A program with the items of `TEMPORARY_DECLS`, the items `extra`, and a `main` with some
/// values, then the statements `body`.
fn temporary_program(extra: &str, body: &str) -> String {
    format!(
        "{TEMPORARY_DECLS}{extra}fn main() {{\n    let o = O(v: 1)\n    let c = num() > 0\n    \
        let x = bad()\n    {body}\n    print(o.v)\n    print(c)\n}}"
    )
}

/// GIVEN `fn bad() -> Nope`, which fails with E0030, and a value that is no place as a `mut`
///   argument, a call, an operator, an `if`, a block or an index of a call, for a function, a
///   generic function, a method, a requirement, a safe call, a written type argument and a
///   `return`
/// WHEN the call is checked
/// THEN the value is E0169 beside the E0030, with the other error of its row if it has one
#[test]
#[expect(clippy::too_many_lines, reason = "a table of rows, one for each form")]
fn temporary_mut_argument_should_be_e0169_beside_an_earlier_error() {
    let rows: &[Row] = &[
        ("a", "", "set(mut bad())", &[(E30, "Nope"), (E169, "bad()")]),
        (
            "b: an operator",
            "",
            "set(mut bad() + 1)",
            &[(E30, "Nope"), (E169, "bad() + 1")],
        ),
        (
            "f: an operator on a local that failed",
            "",
            "set(mut x + 1)",
            &[(E30, "Nope"), (E169, "x + 1")],
        ),
        (
            "g: a generic function",
            "",
            "take(mut bad())",
            &[(E30, "Nope"), (E169, "bad()")],
        ),
        (
            "h: a method",
            "",
            "o.set(mut bad())",
            &[(E30, "Nope"), (E169, "bad()")],
        ),
        (
            "m: an `if` that takes its type from its context",
            "",
            "take(mut if c { [] } else { [] })",
            &[(E30, "Nope"), (E169, "if c { [] } else { [] }")],
        ),
        (
            "j: an unknown function, beside the E0030",
            "",
            "set(mut nope())",
            &[
                (E30, "Nope"),
                (Code::FunctionNotFound, "nope"),
                (E169, "nope()"),
            ],
        ),
        (
            "idx: an index of a call",
            "",
            "set(mut bad()[0])",
            &[(E30, "Nope"), (E169, "bad()[0]")],
        ),
        (
            "block: a block",
            "",
            "set(mut { bad() })",
            &[(E30, "Nope"), (E169, "{ bad() }")],
        ),
        (
            "blocklit: a block of a literal",
            "",
            "take(mut { [] })",
            &[(E30, "Nope"), (E169, "{ [] }")],
        ),
        (
            "coal: `??` on a local that failed",
            "",
            "set(mut x ?? 1)",
            &[(E30, "Nope"), (E169, "x ?? 1")],
        ),
        (
            "paren: a call in parentheses",
            "",
            "set(mut (bad()))",
            &[(E30, "Nope"), (E169, "(bad())")],
        ),
        (
            "retid: after `return`",
            "fn g() -> i64 { return id(mut bad()) }\n",
            "print(g())",
            &[(E30, "Nope"), (E169, "bad()")],
        ),
        (
            "written: a written type argument",
            "",
            "take<i64>(mut bad())",
            &[(E30, "Nope"), (E169, "bad()")],
        ),
        (
            "safe: a safe call",
            "",
            "let oo: O? = Some(o)\n    oo?.set(mut bad())",
            &[(E30, "Nope"), (E169, "bad()")],
        ),
        (
            "nested: a `mut` call in a `mut` argument",
            "",
            "set(mut id(mut bad()))",
            &[(E30, "Nope"), (E169, "id(mut bad())"), (E169, "bad()")],
        ),
        (
            "req: a requirement, through `any J` and through `U: J`",
            "fn via_any(j: any J) { j.put(mut bad()) }\n\
            fn via_gen<U: J>(j: U) { j.put(mut bad()) }\n",
            "via_any(o)\n    via_gen(o)",
            &[(E30, "Nope"), (E169, "bad()"), (E169, "bad()")],
        ),
    ];
    check_rows(rows, temporary_program);
}

/// GIVEN a value that is no place as a `mut` argument, and the value has an error of its own,
///   with no earlier error: an unknown function, a bad operand, a name that failed to bind, an
///   index of a call, `x is O`, or `self` outside a method or an index of it in a method
/// WHEN the call is checked
/// THEN the error of the value stands, and the value is E0169 too: it is no place whatever its
///   type
#[test]
fn temporary_mut_argument_with_its_own_error_should_also_be_e0169() {
    let set = "fn set(mut n: i64) { n = 1 }\n";
    let alone = [
        (
            "own-a: an unknown function",
            "fn main() {\n    set(mut nope())\n}",
            vec![(Code::FunctionNotFound, "nope"), (E169, "nope()")],
        ),
        (
            "own-b: an operand of another type",
            "fn num() -> i64 { 1 }\nfn main() {\n    set(mut num() + \"a\")\n}",
            vec![(Code::Mismatch, "num()"), (E169, "num() + \"a\"")],
        ),
        (
            "own-c: a name that failed to bind",
            "fn main() {\n    let s = nope\n    set(mut s.f())\n}",
            vec![(Code::BindingNotFound, "nope"), (E169, "s.f()")],
        ),
        (
            "self-out: an index of `self` outside a method",
            "fn main() {\n    set(mut self[0])\n}",
            vec![(Code::SelfOutsideMethod, "self"), (E169, "self[0]")],
        ),
        (
            "selfcall-alone: a call on `self` outside a method",
            "fn main() {\n    set(mut self.f())\n}",
            vec![(Code::SelfOutsideMethod, "self"), (E169, "self.f()")],
        ),
    ];
    for (row, main, wanted) in alone {
        let src = format!("{set}{main}");
        assert_eq!(error_texts(&src), wanted, "row `{row}`");
    }
    let beside: &[Row] = &[
        (
            "fncall: an index of a call",
            "",
            "set(mut num()[0])",
            &[
                (E30, "Nope"),
                (Code::CannotIndex, "num()"),
                (E169, "num()[0]"),
            ],
        ),
        (
            "isx: `x is O`",
            "",
            "take(mut x is O)",
            &[(E30, "Nope"), (E169, "x is O"), (Code::TypeAsValue, "O")],
        ),
        (
            "inmeth: an index of `self` in a method",
            "class P { v: i64; fn f(self) { set(mut self[0]) } }\n",
            "print(0)",
            &[
                (E30, "Nope"),
                (Code::CannotIndex, "self"),
                (E169, "self[0]"),
            ],
        ),
        (
            "selfidx2: an index of an index of `self` outside a method",
            "",
            "set(mut self[0][0])",
            &[
                (E30, "Nope"),
                (Code::SelfOutsideMethod, "self"),
                (E169, "self[0][0]"),
            ],
        ),
    ];
    check_rows(beside, temporary_program);
}

/// GIVEN a `mut` argument that is a place, a range or `self`, or a value that a rule other than
///   the place rule handles, and its base, its bound or itself has an earlier error
/// WHEN the call is checked
/// THEN the reports are those of the call before: a place with a failed base has no E0169, as its
///   report needs the type, and the other forms keep what they had
#[test]
#[expect(clippy::too_many_lines, reason = "a table of rows, one for each form")]
fn mut_argument_beside_an_earlier_error_should_keep_its_reports() {
    let rows: &[Row] = &[
        (
            "i: a field of a call",
            "",
            "set(mut bad().v)",
            &[(E30, "Nope")],
        ),
        (
            "n: a field of a local that failed",
            "",
            "set(mut x.v)",
            &[(E30, "Nope")],
        ),
        (
            "o: a range of a local that failed",
            "",
            "set(mut x[0..1])",
            &[(E30, "Nope")],
        ),
        (
            "rng: a range of a call",
            "",
            "sets(mut bad()[0..1])",
            &[(E30, "Nope")],
        ),
        (
            "lbl: a labeled argument",
            "",
            "set(n: mut bad())",
            &[(E30, "Nope"), (Code::LabeledArgument, "n")],
        ),
        (
            "fnval: a function",
            "",
            "set(mut num)",
            &[(E30, "Nope"), (Code::TypeAsValue, "num")],
        ),
        (
            "fnfield: a field of a function",
            "",
            "set(mut num.v)",
            &[(E30, "Nope"), (Code::TypeAsValue, "num")],
        ),
        (
            "selfv: a field of `self` outside a method",
            "",
            "set(mut self.v)",
            &[(E30, "Nope"), (Code::SelfOutsideMethod, "self")],
        ),
        (
            "match: a `match`",
            "",
            "set(mut match c {\n        true => bad()\n        false => 1\n    })",
            &[
                (E30, "Nope"),
                (
                    E169,
                    "match c {\n        true => bad()\n        false => 1\n    }",
                ),
            ],
        ),
        (
            "interp: an interpolation",
            "",
            "sets(mut \"{bad()}\")",
            &[(E30, "Nope"), (E169, "\"{bad()}\"")],
        ),
        (
            "cast: a cast",
            "",
            "set(mut i64(bad()))",
            &[(E30, "Nope"), (E169, "i64(bad())")],
        ),
        (
            "neg: a negation",
            "",
            "set(mut -bad())",
            &[(E30, "Nope"), (E169, "-bad()")],
        ),
        (
            "not: a `!`",
            "",
            "take(mut !x)",
            &[(E30, "Nope"), (E169, "!x")],
        ),
        (
            "cmp: a comparison",
            "",
            "take(mut x == 1)",
            &[(E30, "Nope"), (E169, "x == 1")],
        ),
        (
            "nonelit: `None`",
            "",
            "take(mut None)",
            &[(E30, "Nope"), (E169, "None")],
        ),
        (
            "somelit: `Some` of an empty array",
            "",
            "take(mut Some([]))",
            &[(E30, "Nope"), (E169, "Some([])")],
        ),
        (
            "strlit: a string",
            "",
            "sets(mut \"a\")",
            &[(E30, "Nope"), (E169, "\"a\"")],
        ),
        (
            "c: a call",
            "",
            "set(mut num())",
            &[(E30, "Nope"), (E169, "num()")],
        ),
        (
            "d: an `if`",
            "",
            "set(mut if c { 1 } else { 2 })",
            &[(E30, "Nope"), (E169, "if c { 1 } else { 2 }")],
        ),
        (
            "e: a literal",
            "",
            "set(mut 1)",
            &[(E30, "Nope"), (E169, "1")],
        ),
        (
            "k: a constructor",
            "",
            "take(mut O(v: 1))",
            &[(E30, "Nope"), (E169, "O(v: 1)")],
        ),
        (
            "l: an empty array",
            "",
            "take(mut [])",
            &[(E30, "Nope"), (E169, "[]")],
        ),
        (
            "p: a call that returns a value",
            "",
            "take(mut num())",
            &[(E30, "Nope"), (E169, "num()")],
        ),
    ];
    check_rows(rows, temporary_program);
    // `self` in a method of an extension is E0155 first, and stops the call.
    let src = "fn set(mut n: i64) { n = 1 }\nextend [i64] {\n    fn f(self) {\n        \
        set(mut self[0])\n    }\n}\nfn main() {\n    [1].f()\n}";
    assert_eq!(error_texts(src), [(Code::CannotExtend, "[i64]")], "self-in");
}

/// GIVEN `fn f(mut x: Nope)`, whose parameter type fails with E0030, and `let mut a = 1`
/// WHEN `main` calls `f(mut a)`
/// THEN the E0030 of the declaration is the one error: the argument has no error of its own
#[test]
fn a_mut_argument_for_a_parameter_with_no_type_should_report_only_the_declaration() {
    let src = "fn f(mut x: Nope) {}\nfn main() {\n    let mut a = 1\n    f(mut a)\n}";
    codes(src, &[(Code::NeverReassigned, (1, 10)), (E30, (1, 13))]);
}
