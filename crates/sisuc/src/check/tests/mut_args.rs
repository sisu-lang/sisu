//! `mut` arguments: the place, its type, the overlap of two places, the `MutCall` node, and the
//! warnings of a local that is passed `mut`.

use super::*;
use crate::check::mut_args::MUT_KEY_STEPS;

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
        fn a(mut p: P, mut xs: [i64]) {\n    xs[0] = 1\n    p.x = xs\n}\n\
        fn b(mut xs: [i64], mut p: P) {\n    xs[0] = 1\n    p.x = xs\n}\n\
        fn c(mut i: Inner, mut xs: [i64]) {\n    xs[0] = 1\n    i.cells = xs\n}\n\
        fn d(mut xs: [i64], mut i: Inner) {\n    xs[0] = 1\n    i.cells = xs\n}\n\
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
            "fn f(mut a: [Point], mut b: Point) {\n    b.x = 1\n    a[0] = b\n}\n\
            fn main() {\n    let mut ps = [Point(x: 1)]\n    f(mut ps, mut ps[0])\n    print(ps[0].x)\n}",
        ),
        (
            "ps[0], ps",
            "fn f(mut a: Point, mut b: [Point]) {\n    a.x = 1\n    b[0] = a\n}\n\
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

/// GIVEN a `mut` argument `mut self`, `mut self[0]` or `mut self[0..1]` outside a method
/// WHEN it is checked
/// THEN it is E0041 only: there is no `self` to pass, so no E0169
#[test]
fn mut_self_outside_a_method_should_be_reported_once() {
    for place in ["self", "self[0]", "self[0..1]"] {
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

/// GIVEN a `mut` argument that is a value, or a range index, and whose expression has an error
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
        ("call of an unknown function", "set(mut nope())"),
        ("index of `self`", "set(mut self[0])"),
        (
            "range of a string that failed to bind",
            "let mut s: string = nope\n    set(mut s[0..1])",
        ),
        (
            "range of a string that failed to concatenate",
            "let s = \"a\" + 1\n    set(mut s[0..1])",
        ),
        (
            "range of an element of an array that failed to bind",
            "let s: [string] = nope\n    set(mut s[0][0..1])",
        ),
        (
            "range of a name that failed to bind",
            "let s = nope\n    set(mut s[0..1])",
        ),
        (
            "call on a name that failed to bind",
            "let s = nope\n    set(mut s.f())",
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
    let rows: [(&str, String, &[Code]); 23] = [
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
///   function, a method, a bound method, an `any I` method, a call with explicit type arguments,
///   and a call nested in another generic call
/// WHEN it is checked
/// THEN it is E0166 only: the value binds `T` as any argument does, so `T` is no cascade
#[test]
fn unmarked_value_at_a_generic_mut_parameter_should_give_the_missing_marker_only() {
    let decls = "fn id<T>(x: T) -> T {\n    x\n}\nfn g<T>(mut x: T) -> T {\n    x\n}\n\
        fn h<T>(a: T, b: T) -> T {\n    a\n}\n\
        interface Gt {\n    fn g<T>(self, mut x: T) -> T\n}\n\
        class C: Gt {\n    n: i64\n    fn g<T>(self, mut x: T) -> T {\n        x\n    }\n}\n";
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
            "let mut c = 2\n    let b: u8 = 1\n    print(h(g(c), b))",
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
    let any = format!(
        "{decls}fn main() {{\n    let f: any Gt = C(n: 1)\n    let r = f.g(1)\n    print(r)\n}}"
    );
    assert_eq!(
        found_codes(&any),
        [Code::MutMarkerMissing],
        "`any I` method"
    );
}

/// GIVEN a place that a generic call fails before it checks its arguments, with a wrong
///   type-argument list
/// WHEN the place has no `mut`
/// THEN E0166 follows the type-argument error, and the fixture `E0166-type-arguments` pins that it
///   has no help
#[test]
fn unmarked_place_of_a_failed_generic_call_should_still_need_its_marker() {
    for call in ["rw<i64, i64>(x, 5)", "rw<Nope>(x, 5)"] {
        let src = format!(
            "fn rw<T>(mut x: T, v: T) {{\n    x = v\n}}\nfn main() {{\n    let mut x = 1\n    {call}\n    print(x)\n}}"
        );
        let found = found_codes(&src);
        assert_eq!(
            found.last(),
            Some(&Code::MutMarkerMissing),
            "`{call}`: {found:?}"
        );
    }
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
