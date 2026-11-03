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
