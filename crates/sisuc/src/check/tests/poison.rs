//! The poison rule. A poisoned value has a type only when the source states it apart from the
//! failed part. Each reader reads a typed poisoned value as any value of that type.

use std::iter;

use super::*;
use Code::{
    AlreadyConverted, ArgumentCount, AssignToImmutable, BindingNotFound, BraceOnNewLine,
    CannotInfer, CoalesceNotOptional, FunctionNotFound, JumpOutsideLoop, LabeledArgument,
    MayBeNone, Mismatch, MissingField, NeverNone, NoMember, NotConform, SafeAccessNotOptional,
    TooManyArguments, TypeArguments, TypeAsValue, UnknownType, UnwrapNeedsOptional,
};

/// The declarations C: the classes, enums and functions that each row of `c_rows` uses. `Broken`
/// has its `{` on a line of its own, so the parser marks it poisoned. The statements of a row go
/// in the body of `f`, from line 41.
const C: &str = "class P {
    name: string
    v: i64
    fn m(self) -> i64 { 1 }
    fn u(self) {}
    fn gm<T>(self, a: T) -> i64 { 1 }
    fn gn<T>(self, a: T) -> T { a }
}
class Q {
    n: i64
}
class Box<T> {
    v: T
    fn other<U>(self, a: U) -> T { self.v }
}
interface D {
    fn dm(self) -> i64
}
class Rock {}
enum E {
    A
    V(i64)
    fn k(self, x: i64) -> i64 { x }
}
enum G<T> {
    W(T)
    X
}
enum Broken
{ A }
fn g(x: i64) {}
fn h(x: i64) -> i64 { x }
fn two(a: i64, b: i64) -> i64 { a }
fn keep<T>(a: T, b: i64) -> i64 { b }
fn pick<T>(a: T, b: i64) -> T { a }
fn both<T>(a: T, b: T) -> T { a }
fn id<V>(v: V) -> V { v }
fn bd<T: D>(a: T, b: i64) -> T { a }
fn arr<T>(a: T) -> [i64] { [] }
fn f(p: P?, o: i64?, c: bool, k: i64, mut xs: [i64], mut s: P, e: E, bi: Box<i64>) -> i64 {
";

/// The parser's E0004 at the `{` of `Broken`, which each program of `c_rows` gives first.
const BROKEN: Wanted = (BraceOnNewLine, (30, 1));

/// The declarations C, with `stmt` in the body of `f`: each part between ` and `, and each line
/// of a part, is one line. `kb` comes after `f`, so it moves no line of the body.
fn c_program(stmt: &str) -> String {
    let body = stmt
        .split(" and ")
        .flat_map(|part| part.split('\n'))
        .fold(String::new(), |body, line| body + "    " + line + "\n");
    format!("{C}{body}    return 1\n}}\nfn kb<T: D>(a: T) -> i64 {{ 1 }}\nfn main() {{}}\n")
}

/// `error_rows` over the declarations C: the program of each row is the `c_program` of its
/// statements. Each row must give the E0004 of `Broken` before its own errors.
fn c_rows(rows: &[ErrorRow<'_>]) {
    let programs: Vec<ErrorRow<'_>> = rows
        .iter()
        .map(|(name, stmt, wanted)| {
            let wanted = iter::once(BROKEN).chain(wanted.iter().copied()).collect();
            (*name, c_program(stmt), wanted)
        })
        .collect();
    error_rows(&programs);
}

/// A `.` on the optional `p` fails: E0062 is the one error of each reader, and a true error
/// beside it stays.
#[test]
#[expect(clippy::too_many_lines, reason = "one row for each row of S12")]
fn a_failed_chain_should_report_only_its_true_errors() {
    c_rows(&[
        (
            "a declared let",
            "let _a: i64 = p.name".into(),
            vec![(MayBeNone, (41, 19))],
        ),
        (
            "an inferred let",
            "let _a = p.name".into(),
            vec![(MayBeNone, (41, 14))],
        ),
        (
            "an argument",
            "g(p.name)".into(),
            vec![(MayBeNone, (41, 7))],
        ),
        (
            "a later argument",
            "two(1, p.name)".into(),
            vec![(MayBeNone, (41, 12))],
        ),
        (
            "a method's argument",
            "xs.push(p.name)".into(),
            vec![(MayBeNone, (41, 13))],
        ),
        (
            "an operand as an argument",
            "h(1 + p.name)".into(),
            vec![(MayBeNone, (41, 11))],
        ),
        (
            "a return",
            "return p.name".into(),
            vec![(MayBeNone, (41, 12))],
        ),
        (
            "a condition",
            "if p.name { }".into(),
            vec![(MayBeNone, (41, 8))],
        ),
        (
            "a loop's condition",
            "while p.name { }".into(),
            vec![(MayBeNone, (41, 11))],
        ),
        ("an index", "[1][p.name]".into(), vec![(MayBeNone, (41, 9))]),
        (
            "a range's endpoint",
            "xs[p.name..1]".into(),
            vec![(MayBeNone, (41, 8))],
        ),
        (
            "an element, required",
            "let _a: [i64] = [p.name]".into(),
            vec![(MayBeNone, (41, 22))],
        ),
        (
            "an element, hinted",
            "let _a = [1, p.name]".into(),
            vec![(MayBeNone, (41, 18))],
        ),
        (
            "a nested element",
            "let _a: [[i64]] = [[p.name]]".into(),
            vec![(MayBeNone, (41, 25))],
        ),
        (
            "a branch",
            "let _a = if true { 1 } else { p.name }".into(),
            vec![(MayBeNone, (41, 35))],
        ),
        (
            "a field",
            "let _a = Q(n: p.name)".into(),
            vec![(MayBeNone, (41, 19))],
        ),
        (
            "a payload",
            "let _a = E.V(p.name)".into(),
            vec![(MayBeNone, (41, 18))],
        ),
        (
            "an assignment",
            "let mut a = 1 and a = p.name".into(),
            vec![(MayBeNone, (42, 9))],
        ),
        (
            "a compound assignment",
            "let mut a = 1 and a += p.name".into(),
            vec![(MayBeNone, (42, 10))],
        ),
        (
            "an index assignment",
            "xs[0] = p.name".into(),
            vec![(MayBeNone, (41, 13))],
        ),
        (
            "a field assignment, with its true E0001",
            "s.v = p.name".into(),
            vec![(AssignToImmutable, (41, 5)), (MayBeNone, (41, 11))],
        ),
        (
            "a failed place, with its true E0001",
            "p.name = 1".into(),
            vec![(MayBeNone, (41, 5)), (AssignToImmutable, (41, 5))],
        ),
        (
            "an operator",
            "let _a = p.name + 1".into(),
            vec![(MayBeNone, (41, 14))],
        ),
        (
            "a wrong other operand",
            "let _a = p.name + true".into(),
            vec![(MayBeNone, (41, 14))],
        ),
        (
            "a right operand",
            "let _a = 1.5 + p.name".into(),
            vec![(MayBeNone, (41, 20))],
        ),
        (
            "a comparison",
            "let _a = p.name < 1".into(),
            vec![(MayBeNone, (41, 14))],
        ),
        (
            "an equality, today E0047",
            "let _a = p.name == 1".into(),
            vec![(MayBeNone, (41, 14))],
        ),
        (
            "a logical operator",
            "let _a = p.name && true".into(),
            vec![(MayBeNone, (41, 14))],
        ),
        (
            "a negation",
            "let _a = -p.name".into(),
            vec![(MayBeNone, (41, 15))],
        ),
        (
            "a not",
            "let _a = !p.name".into(),
            vec![(MayBeNone, (41, 15))],
        ),
        (
            "a method on a poisoned value",
            "let _a = p.name.len()".into(),
            vec![(MayBeNone, (41, 14))],
        ),
        (
            "a member",
            "let _a = p.name.nope".into(),
            vec![(MayBeNone, (41, 14))],
        ),
        (
            "an unknown method",
            "let _a = p.name.nope()".into(),
            vec![(MayBeNone, (41, 14))],
        ),
        (
            "a `?.` member",
            "let _a = p.name?.nope".into(),
            vec![(MayBeNone, (41, 14))],
        ),
        (
            "a match subject",
            "match p.name { _ => {} }".into(),
            vec![(MayBeNone, (41, 11))],
        ),
        (
            "interpolation",
            r#"print("{p.name}")"#.into(),
            vec![(MayBeNone, (41, 13))],
        ),
        (
            "a loop",
            "for x in p.name { print(x) }".into(),
            vec![(MayBeNone, (41, 14))],
        ),
        (
            "a range",
            "for x in 0..p.name { print(x) }".into(),
            vec![(MayBeNone, (41, 17))],
        ),
        (
            "a conversion",
            "let _a = i64(p.name)".into(),
            vec![(MayBeNone, (41, 18))],
        ),
        (
            "a conversion, today E0054",
            "let _a = string(p.name)".into(),
            vec![(MayBeNone, (41, 21))],
        ),
        (
            "a `??`, left",
            "let _a = p.name ?? 1".into(),
            vec![(MayBeNone, (41, 14))],
        ),
        (
            "a `??`, right",
            "let _a = o ?? p.name".into(),
            vec![(MayBeNone, (41, 19))],
        ),
        (
            "a method on `P?`",
            r#"let _a: string = "x" + p.m()"#.into(),
            vec![(MayBeNone, (41, 28))],
        ),
        (
            "a true error beside a poisoned value",
            "let _a = true + p.name".into(),
            vec![(Mismatch, (41, 14)), (MayBeNone, (41, 21))],
        ),
    ]);
}

/// Each source of a poisoned value at each reader: a required type, an operator, a member, an
/// argument and a condition. `h()` states its type, `i64`, so its later errors stay.
#[test]
#[expect(clippy::too_many_lines, reason = "one row for each cell of S13")]
fn each_source_should_obey_the_rule_at_each_reader() {
    c_rows(&[
        (
            "nothing() required",
            "let _a: i64 = nothing()".into(),
            vec![(FunctionNotFound, (41, 19))],
        ),
        (
            "nothing() operator",
            "let _a = nothing() + true".into(),
            vec![(FunctionNotFound, (41, 14))],
        ),
        (
            "nothing() member",
            "let _a = nothing().nope".into(),
            vec![(FunctionNotFound, (41, 14))],
        ),
        (
            "nothing() argument",
            "g(nothing())".into(),
            vec![(FunctionNotFound, (41, 7))],
        ),
        (
            "nothing() condition",
            "if nothing() { }".into(),
            vec![(FunctionNotFound, (41, 8))],
        ),
        (
            "missing required",
            "let _a: i64 = missing".into(),
            vec![(BindingNotFound, (41, 19))],
        ),
        (
            "missing operator",
            "let _a = missing + true".into(),
            vec![(BindingNotFound, (41, 14))],
        ),
        (
            "missing member",
            "let _a = missing.nope".into(),
            vec![(BindingNotFound, (41, 14))],
        ),
        (
            "missing argument",
            "g(missing)".into(),
            vec![(BindingNotFound, (41, 7))],
        ),
        (
            "missing condition",
            "if missing { }".into(),
            vec![(BindingNotFound, (41, 8))],
        ),
        (
            "id(nothing()) required",
            "let _a: i64 = id(nothing())".into(),
            vec![(FunctionNotFound, (41, 22))],
        ),
        (
            "id(nothing()) operator",
            "let _a = id(nothing()) + true".into(),
            vec![(FunctionNotFound, (41, 17))],
        ),
        (
            "id(nothing()) member",
            "let _a = id(nothing()).nope".into(),
            vec![(FunctionNotFound, (41, 17))],
        ),
        (
            "id(nothing()) argument",
            "g(id(nothing()))".into(),
            vec![(FunctionNotFound, (41, 10))],
        ),
        (
            "id(nothing()) condition",
            "if id(nothing()) { }".into(),
            vec![(FunctionNotFound, (41, 11))],
        ),
        ("Broken required", "let _a: i64 = Broken".into(), vec![]),
        ("Broken operator", "let _a = Broken + true".into(), vec![]),
        ("Broken member", "let _a = Broken.nope".into(), vec![]),
        ("Broken argument", "g(Broken)".into(), vec![]),
        ("Broken condition", "if Broken { }".into(), vec![]),
        ("Broken() required", "let _a: i64 = Broken()".into(), vec![]),
        (
            "Broken() operator",
            "let _a = Broken() + true".into(),
            vec![],
        ),
        ("Broken() member", "let _a = Broken().nope".into(), vec![]),
        ("Broken() argument", "g(Broken())".into(), vec![]),
        ("Broken() condition", "if Broken() { }".into(), vec![]),
        ("Broken.x required", "let _a: i64 = Broken.x".into(), vec![]),
        (
            "Broken.x operator",
            "let _a = Broken.x + true".into(),
            vec![],
        ),
        ("Broken.x member", "let _a = Broken.x.nope".into(), vec![]),
        ("Broken.x argument", "g(Broken.x)".into(), vec![]),
        ("Broken.x condition", "if Broken.x { }".into(), vec![]),
        (
            "h() required",
            "let _a: i64 = h()".into(),
            vec![(ArgumentCount, (41, 19))],
        ),
        (
            "h() operator",
            "let _a = h() + true".into(),
            vec![(ArgumentCount, (41, 14)), (Mismatch, (41, 20))],
        ),
        (
            "h() member",
            "let _a = h().nope".into(),
            vec![(ArgumentCount, (41, 14)), (NoMember, (41, 18))],
        ),
        (
            "h() argument",
            "g(h())".into(),
            vec![(ArgumentCount, (41, 7))],
        ),
        (
            "h() condition",
            "if h() { }".into(),
            vec![(ArgumentCount, (41, 8)), (Mismatch, (41, 8))],
        ),
        (
            "p.m() required",
            "let _a: i64 = p.m()".into(),
            vec![(MayBeNone, (41, 19))],
        ),
        (
            "p.m() operator",
            "let _a = p.m() + true".into(),
            vec![(MayBeNone, (41, 14))],
        ),
        (
            "p.m() member",
            "let _a = p.m().nope".into(),
            vec![(MayBeNone, (41, 14))],
        ),
        (
            "p.m() argument",
            "g(p.m())".into(),
            vec![(MayBeNone, (41, 7))],
        ),
        (
            "p.m() condition",
            "if p.m() { }".into(),
            vec![(MayBeNone, (41, 8))],
        ),
    ]);
}

/// The lines of the fixture, each with the errors whose primary line is that line. A typed
/// poisoned `bool` or `string` gets the true E0048 of `??`; an untyped value gets none.
#[test]
fn a_chain_fixture_should_read_a_poisoned_value_as_any_reader() {
    let src = include_str!("../../../tests/diagnostics/E0062-chain-no-cascade.sisu");
    let (_, ds) = checked_with(PRELUDE_SOURCE, src);
    let found = errors(src, &ds);
    let lines: [(&str, usize, &[Wanted]); 20] = [
        (
            "(c.v < 1) ?? false",
            122,
            &[(CoalesceNotOptional, (122, 11)), (MayBeNone, (122, 12))],
        ),
        (
            "(!c.b) ?? false",
            130,
            &[(CoalesceNotOptional, (130, 11)), (MayBeNone, (130, 13))],
        ),
        (
            "(c.b && k) ?? false",
            134,
            &[(CoalesceNotOptional, (134, 11)), (MayBeNone, (134, 12))],
        ),
        (
            "(if c.b { 1 } else { 2 }) ?? 0",
            175,
            &[(CoalesceNotOptional, (175, 11)), (MayBeNone, (175, 15))],
        ),
        ("(-c.v) ?? 0", 126, &[(MayBeNone, (126, 13))]),
        (
            "(if k { c.v } else { 1 }) ?? 0",
            155,
            &[(MayBeNone, (155, 19))],
        ),
        (
            "(match p { A => c.v; B => 1 }) ?? 0",
            159,
            &[(MayBeNone, (159, 27))],
        ),
        (
            "(if k { if k { c.v } else { 1 } } else { 1 }) ?? 0",
            163,
            &[(MayBeNone, (163, 26))],
        ),
        (
            "(if k { 1 } else if k { c.v } else { 1 }) ?? 0",
            167,
            &[(MayBeNone, (167, 35))],
        ),
        (
            "(match p { A => if k { c.v } else { 1 }; B => 1 }) ?? 0",
            171,
            &[(MayBeNone, (171, 34))],
        ),
        (
            "(if let x = c.v { x } else { 0 }) ?? 0",
            318,
            &[(MayBeNone, (318, 23))],
        ),
        (
            "(c.v == 1) ?? false",
            210,
            &[(CoalesceNotOptional, (210, 11)), (MayBeNone, (210, 12))],
        ),
        (
            "\"{c.v}\" ?? \"\"",
            214,
            &[(CoalesceNotOptional, (214, 11)), (MayBeNone, (214, 13))],
        ),
        ("[c.v] ?? [1]", 218, &[(MayBeNone, (218, 12))]),
        ("Bx().wrap(c.v) ?? 0", 196, &[(MayBeNone, (196, 21))]),
        ("dbg(c.v) ?? 0", 290, &[(MayBeNone, (290, 15))]),
        ("x.get(c.v) ?? 0", 302, &[(MayBeNone, (302, 17))]),
        (
            "match c.v { Some(\"a\") => {}; _ => {} }",
            254,
            &[(MayBeNone, (254, 11))],
        ),
        (
            "match c.v { Some(Some(x)) => {}; _ => {} }",
            258,
            &[(MayBeNone, (258, 11))],
        ),
        ("match c.e { A => {} }", 274, &[(MayBeNone, (274, 11))]),
    ];
    let mut red = Vec::new();
    for (form, line, wanted) in lines {
        let here: Vec<Wanted> = found
            .iter()
            .copied()
            .filter(|&(_, (at_line, _))| at_line == line)
            .collect();
        if here != wanted {
            red.push(format!(
                "line {line} `{form}`: found {here:?}, wanted {wanted:?}"
            ));
        }
    }
    assert!(
        red.is_empty(),
        "{} red lines:\n{}",
        red.len(),
        red.join("\n")
    );
}

/// A place, an element or a `push` below a failed `.` is untyped. So the value is checked with no
/// expected type, and its mismatch waits. The E0001 of a field below the untyped member goes too:
/// no field is looked up.
#[test]
fn a_place_under_a_poisoned_member_should_be_untyped() {
    let lines = "    c.inner.v = \"a\"\n    c.inner.xs.push(\"a\")\n    c.inner.xs[0] = \"a\"\n";
    let mutable = format!(
        "class In {{\n    mut v: i64\n    mut xs: [i64]\n}}\nclass C {{\n    mut inner: In\n}}\nfn f(mut c: C?) {{\n{lines}}}\nfn main() {{}}\n"
    );
    let fixed = format!(
        "class In {{\n    v: i64\n    xs: [i64]\n}}\nclass C {{\n    inner: In\n}}\nfn f(c: C?) {{\n{lines}}}\nfn main() {{}}\n"
    );
    let three = vec![
        (MayBeNone, (9, 5)),
        (MayBeNone, (10, 5)),
        (MayBeNone, (11, 5)),
    ];
    // `xs[0]` and `yss[0][1]` are `i64`, so no `push` is on a field; `yss[0]` is an array.
    let pushes = "class N {\n    xs: [i64]\n    yss: [[i64]]\n}\nfn f(c: N?) {\n    c.xs[0].push(1)\n    c.yss[0][1].push(1)\n    c.yss[0].push(1)\n}\nfn main() {}\n";
    error_rows(&[
        ("fields that can change", mutable, three.clone()),
        ("fields that cannot change", fixed, three),
        (
            "a push past the indexes",
            pushes.to_owned(),
            vec![
                (MayBeNone, (6, 5)),
                (MayBeNone, (7, 5)),
                (MayBeNone, (8, 5)),
                (AssignToImmutable, (8, 5)),
            ],
        ),
    ]);
}

/// The E0001 of a `push` needs a field that a failed `.` names, through indexes that do not fail
/// (plan call 2). A `?.push` on that field keeps it, as a `.push` does.
#[test]
fn a_push_should_report_e0001_only_on_a_field_that_a_poisoned_member_names() {
    let program = |line: &str| {
        format!(
            "class N {{\n    yss: [[i64]]\n    oxs: [i64]?\n    oyss: [[i64]?]\n}}\nfn f(c: N?, n: N) {{\n    {line}\n}}\nfn main() {{}}\n"
        )
    };
    error_rows(&[
        (
            "no poisoned member, a string index",
            program("n.yss[\"a\"].push(1)"),
            vec![(Mismatch, (7, 11))],
        ),
        (
            "no poisoned member, a bool index",
            program("n.yss[true].push(1)"),
            vec![(Mismatch, (7, 11))],
        ),
        (
            "a root `?.`, a string index",
            program("c?.yss[\"a\"].push(1)"),
            vec![(MayBeNone, (7, 5)), (Mismatch, (7, 12))],
        ),
        (
            "a poisoned member, a string index",
            program("c.yss[\"a\"].push(1)"),
            vec![(MayBeNone, (7, 5)), (Mismatch, (7, 11))],
        ),
        (
            "a `?.push` on an optional field",
            program("c.oxs?.push(1)"),
            vec![(MayBeNone, (7, 5)), (AssignToImmutable, (7, 5))],
        ),
        (
            "a `?.push` on an optional element",
            program("c.oyss[0]?.push(1)"),
            vec![(MayBeNone, (7, 5)), (AssignToImmutable, (7, 5))],
        ),
        (
            "a `?.push` of a string on an optional field",
            program("c.oxs?.push(\"a\")"),
            vec![(MayBeNone, (7, 5)), (AssignToImmutable, (7, 5))],
        ),
    ]);
}

/// `x` binds a poisoned member, so `x` is untyped, and so is the `if let` that gives it.
#[test]
fn an_untyped_binding_should_fail_its_branch() {
    c_rows(&[(
        "if let of a poisoned member",
        "let a = if let x = p.v { x + 1 } else { 0 } and print(a + true)".into(),
        vec![(MayBeNone, (41, 24))],
    )]);
}

/// An untyped local names no field, so a `push` on it adds no E0001.
#[test]
fn a_push_on_an_untyped_local_should_add_no_error() {
    c_rows(&[
        (
            "a poisoned member",
            "let a = p.name and a.push(1)".into(),
            vec![(MayBeNone, (41, 13))],
        ),
        (
            "a failed call",
            "let a = nothing() and a.push(1)".into(),
            vec![(FunctionNotFound, (41, 13))],
        ),
        (
            "an unknown name, past an index",
            "let a = nope and a[0].push(1)".into(),
            vec![(BindingNotFound, (41, 13))],
        ),
    ]);
}

/// `-h()`, `-i64(nope)` and the join with a typed poisoned branch are `i64`, so `a + true` is
/// E0032.
#[test]
fn a_typed_poisoned_operand_should_read_as_its_type() {
    c_rows(&[
        (
            "negation of a failed call",
            "let a = -h() and print(a + true)".into(),
            vec![(ArgumentCount, (41, 14)), (Mismatch, (42, 15))],
        ),
        (
            "negation of a conversion",
            "let a = -i64(nope) and print(a + true)".into(),
            vec![(BindingNotFound, (41, 18)), (Mismatch, (42, 15))],
        ),
        (
            "a typed poisoned branch",
            "let a = if c { 1 } else { h() } and print(a + true)".into(),
            vec![(ArgumentCount, (41, 31)), (Mismatch, (42, 15))],
        ),
    ]);
}

/// A member behind `?.` keeps its type, and a `?.` or an `if let` after a failed `.` adds no
/// error.
#[test]
fn a_safe_member_should_keep_its_type() {
    c_rows(&[
        (
            "safe member",
            "let _a: i64 = p?.name?.len()".into(),
            vec![(Mismatch, (41, 19))],
        ),
        (
            "safe chain",
            "let _a = p?.name + 1".into(),
            vec![(Mismatch, (41, 14))],
        ),
        (
            "if let member",
            "if let x = p.name { print(x + true) }".into(),
            vec![(MayBeNone, (41, 16))],
        ),
        (
            "safe after failed",
            "let _a = p.name?.len()".into(),
            vec![(MayBeNone, (41, 14))],
        ),
        (
            "typed condition",
            "if p.v < 1 { }".into(),
            vec![(MayBeNone, (41, 8))],
        ),
    ]);
}

/// `body` on line 11 of `fn g` over `c: Outer?`, `i: Inner?`, `o: [i64]?` and the plain `p: Outer`.
/// The classes have no `mut` field, so a write through them is E0001.
fn with_body(body: &str) -> String {
    format!(
        "class Inner {{\n    v: i64\n    ys: [i64]\n    fn one(self) -> i64 {{ self.v }}\n}}\nclass Outer {{\n    inner: Inner\n    oys: [[i64]]?\n}}\nfn g(c: Outer?, i: Inner?, o: [i64]?, p: Outer) {{\n    {body}\n}}\nfn main() {{}}"
    )
}

/// Each row is a body and the codes that it reports, in order, beside E0062 at its first place.
fn rows(rows: &[(&str, &[Wanted])]) {
    for &(body, wanted) in rows {
        codes_row(body, &with_body(body), wanted);
    }
}

/// A field of an optional is untyped, so no mismatch and no missing member after it is reported.
/// The spec rows `p.name` (the missing unwrap) and index, member, method (the receiver).
#[test]
fn a_field_of_an_optional_should_be_untyped() {
    rows(&[
        ("let s: string = i.v", &[(MayBeNone, (11, 21))]),
        ("let s: string = c.inner.v", &[(MayBeNone, (11, 21))]),
        ("print(c.inner.nope)", &[(MayBeNone, (11, 11))]),
    ]);
}

/// A method of an optional is untyped, so no mismatch and no missing member after it is
/// reported. The spec row `p.m()` (the missing unwrap).
#[test]
fn a_method_of_an_optional_should_be_untyped() {
    rows(&[
        ("let s: string = i.one()", &[(MayBeNone, (11, 21))]),
        ("i.one().nope()", &[(MayBeNone, (11, 5))]),
    ]);
}

/// A member of an optional is untyped, so E0048, E0051, E0044 and E0049 do not report that it
/// is not optional. The spec rows `p.name` and `??`.
#[test]
fn a_poisoned_member_should_get_no_report_that_it_is_not_optional() {
    rows(&[
        ("print(c.inner.v ?? 0)", &[(MayBeNone, (11, 11))]),
        ("print(o.len() ?? 0)", &[(MayBeNone, (11, 11))]),
        (
            "if let x = c.inner.v { print(x) }",
            &[(MayBeNone, (11, 16))],
        ),
        (
            "let x = c.inner.v else { return }",
            &[(MayBeNone, (11, 13))],
        ),
        ("print(c.inner.v == None)", &[(MayBeNone, (11, 11))]),
        ("print(c.inner?.v)", &[(MayBeNone, (11, 11))]),
    ]);
}

/// The same checks on a plain value, which is not optional, so each reports.
#[test]
fn value_that_is_not_optional_should_get_the_report_that_it_is_not_optional() {
    rows(&[
        ("print(p.inner.v ?? 0)", &[(CoalesceNotOptional, (11, 11))]),
        (
            "if let x = p.inner.v { print(x) }",
            &[(UnwrapNeedsOptional, (11, 16))],
        ),
        (
            "let x = p.inner.v else { return }",
            &[(UnwrapNeedsOptional, (11, 13))],
        ),
        ("print(p.inner.v == None)", &[(NeverNone, (11, 11))]),
        ("print(p.inner?.v)", &[(SafeAccessNotOptional, (11, 11))]),
    ]);
}

/// A `let` or a `for` that binds a member of an optional binds no type: the spec rows a binding
/// with no declared type and a `for` variable. The plain value gives the name its type, so a
/// member that it lacks is reported.
#[test]
fn a_name_bound_to_a_poisoned_member_should_be_untyped() {
    rows(&[
        ("let x = c.inner\n    x.nope", &[(MayBeNone, (11, 13))]),
        ("for n in c.inner.ys { n.nope() }", &[(MayBeNone, (11, 14))]),
        ("let x = p.inner\n    x.nope", &[(NoMember, (12, 7))]),
        ("for n in p.inner.ys { n.nope() }", &[(NoMember, (11, 29))]),
    ]);
}

/// An index of an optional array reports E0062, and goes on with the element of the payload.
#[test]
fn index_of_an_optional_array_should_go_on_with_the_element() {
    rows(&[(
        "let s: string = o[0]",
        &[(MayBeNone, (11, 21)), (Mismatch, (11, 21))],
    )]);
}

/// Below a member of an optional, `c.oys[0]` is untyped (the spec row index, member, method).
/// So the argument of the `push` is checked with no expected type, and its mismatch waits. The
/// field `oys` cannot change, so its E0001 stays.
#[test]
fn a_push_on_an_index_of_a_poisoned_member_should_keep_only_its_e0001() {
    rows(&[(
        "c.oys[0].push(\"a\")",
        &[(MayBeNone, (11, 5)), (AssignToImmutable, (11, 5))],
    )]);
}

/// A method of an optional checks its type arguments on the payload. A field is no method.
#[test]
fn type_arguments_on_a_method_of_an_optional_should_be_reported() {
    rows(&[
        (
            "print(i.one<i64>())",
            &[(MayBeNone, (11, 11)), (TypeArguments, (11, 16))],
        ),
        (
            "print(o.len<i64>())",
            &[(MayBeNone, (11, 11)), (TypeArguments, (11, 16))],
        ),
        ("print(i.v<i64>())", &[(TypeArguments, (11, 11))]),
    ]);
}

/// At one start, E0062 comes before E0001, whichever check runs first: an element assignment
/// through `?.`, whose field reports E0001 before the index reports E0062.
#[test]
fn e0062_should_come_before_e0001_for_an_element_assignment_through_safe_access() {
    rows(&[(
        "c?.oys[0] = [1]",
        &[(MayBeNone, (11, 5)), (AssignToImmutable, (11, 5))],
    )]);
}

/// `body` on line 14 of `fn r` over `c: C?`, `k: bool` and `p: E`. `C` holds an `E`, an optional
/// of itself and an array, `mut w`, and a generic method `opt`. `id` and `Bx2` follow `main`.
fn with_cell(body: &str) -> String {
    format!(
        "enum E {{\n    A\n    B\n}}\nclass C {{\n    v: i64\n    e: E\n    ys: [i64]\n    n: C?\n    mut w: i64\n    fn opt<T>(self, t: T) -> T? {{ Some(t) }}\n}}\nfn r(c: C?, k: bool, p: E) {{\n    {body}\n}}\nfn main() {{}}\nfn id<T>(t: T) -> T {{ t }}\nenum Bx2<T> {{\n    Full(T)\n    Empty\n}}"
    )
}

/// `rows` over `with_cell`: each row is a body and the codes that it reports.
fn cell_rows(rows: &[(&str, &[Wanted])]) {
    for &(body, wanted) in rows {
        codes_row(body, &with_cell(body), wanted);
    }
}

/// An operator on a member of an optional is untyped, so its result gets no report that it is
/// not optional. The spec rows `+` with a failed left operand and `??` (either side).
#[test]
fn an_operator_on_a_poisoned_member_should_be_untyped() {
    cell_rows(&[
        ("print((c.v + 1) ?? 0)", &[(MayBeNone, (14, 12))]),
        ("print((c.v + 1) == None)", &[(MayBeNone, (14, 12))]),
        ("print((c.v + 1)?.to_string())", &[(MayBeNone, (14, 12))]),
        (
            "let o: i64? = None\n    print((o ?? c.v) ?? 0)",
            &[(MayBeNone, (15, 17))],
        ),
    ]);
}

/// A block that ends in a member of an optional, and a join with a branch of one, are untyped:
/// the spec rows block (the tail) and `if`, `match` (a branch).
#[test]
fn a_block_and_a_join_of_a_poisoned_member_should_be_untyped() {
    cell_rows(&[
        ("print({ c.v } ?? 0)", &[(MayBeNone, (14, 13))]),
        (
            "print((if k { c.v } else { c.v }) ?? 0)",
            &[(MayBeNone, (14, 19)), (MayBeNone, (14, 32))],
        ),
        (
            "print((if k { c.v } else { 1 }) ?? 0)",
            &[(MayBeNone, (14, 19))],
        ),
    ]);
}

/// A generic call is untyped when only a member of an optional binds its type parameter. A
/// slice of a member of an optional has none. The spec rows of a call and index, member, method.
#[test]
fn a_generic_call_and_a_slice_of_a_poisoned_member_should_be_untyped() {
    cell_rows(&[
        ("print(id(c.v) ?? 0)", &[(MayBeNone, (14, 14))]),
        ("let x = c.ys[0..1]\n    x.nope()", &[(MayBeNone, (14, 13))]),
    ]);
}

/// `==` gives `bool` when an operand fails: the spec row `<`, `==`, `!`, `&&`, `is`. A typed
/// poisoned value reads as any value of its type, so the `??` on it is a true E0048.
#[test]
fn equality_on_a_poisoned_member_should_be_bool() {
    cell_rows(&[(
        "print((c.v == 1) ?? false)",
        &[(CoalesceNotOptional, (14, 11)), (MayBeNone, (14, 12))],
    )]);
}

/// A name that takes all of a member of an optional binds no type: the spec row a binding of a
/// pattern. So `if let` and `let ... else` do not report that it always matches.
#[test]
fn a_name_pattern_on_a_poisoned_member_should_not_always_match() {
    cell_rows(&[
        ("if let x = c.e { print(x) }", &[(MayBeNone, (14, 16))]),
        ("let x = c.e else { return }", &[(MayBeNone, (14, 13))]),
        ("if let _ = c.e { print(1) }", &[(MayBeNone, (14, 16))]),
    ]);
}

/// A member of an optional is untyped, so `Some` and `None` patterns on it are no mistake. A
/// name that takes all of it is untyped: the spec row a binding of a pattern.
#[test]
fn some_and_none_patterns_on_a_poisoned_member_should_be_accepted() {
    cell_rows(&[
        (
            "match c.v { Some(x) => print(x); None => {} }",
            &[(MayBeNone, (14, 11))],
        ),
        (
            "match c.v { None => {}; _ => {} }",
            &[(MayBeNone, (14, 11))],
        ),
        ("match c.v { x => print(x ?? 0) }", &[(MayBeNone, (14, 11))]),
    ]);
}

/// A place through `?.` after a member of an optional is untyped (the spec row index, member,
/// method): no field below the member is looked up, so the write rule finds no field and no
/// E0001 is reported.
#[test]
fn a_place_through_safe_access_after_a_poisoned_member_should_be_untyped() {
    cell_rows(&[
        ("c.n?.ys.push(1)", &[(MayBeNone, (14, 5))]),
        ("c.n?.w = \"a\"", &[(MayBeNone, (14, 5))]),
    ]);
}

/// A generic variant and a call through an interface bound are untyped when only a member of
/// an optional binds their type parameter. The spec row of a call.
#[test]
fn a_generic_variant_and_a_bound_call_of_a_poisoned_member_should_be_untyped() {
    cell_rows(&[(
        "match Bx2.Full(c.v) { Bx2.Full(x) => print(x ?? 0); Bx2.Empty => {} }",
        &[(MayBeNone, (14, 20))],
    )]);
    codes(
        "interface Get {\n    fn get<T>(self, t: T) -> T\n}\nclass C {\n    v: i64\n}\nfn r<X: Get>(x: X, c: C?) {\n    print(x.get(c.v) ?? 0)\n}\nfn main() {}",
        &[(MayBeNone, (8, 17))],
    );
}

/// A name inside a `Some` on a member of an optional is untyped, and the `Some` is no mistake:
/// the subject is untyped (the spec row a binding of a pattern).
#[test]
fn a_name_inside_a_some_on_a_poisoned_member_should_be_untyped() {
    cell_rows(&[
        (
            "match c.v { Some(x) => print(x ?? 0); None => {} }",
            &[(MayBeNone, (14, 11))],
        ),
        (
            "match c.v { Some(x) => x.nope(); None => {} }",
            &[(MayBeNone, (14, 11))],
        ),
        (
            "match Some(c.v) { Some(x) => print(x ?? 0); None => {} }",
            &[(MayBeNone, (14, 16))],
        ),
    ]);
}

/// A member of an optional is untyped, also when its own type is optional. So a name bound to
/// it is untyped, and no later mistake with it is reported. The spec rows `p.name` and a
/// binding of a pattern.
#[test]
fn a_name_bound_to_an_optional_poisoned_member_should_be_untyped() {
    cell_rows(&[
        ("if let x = c.n { x.nope() }", &[(MayBeNone, (14, 16))]),
        (
            "if let x = c.n { print(x.v ?? 0) }",
            &[(MayBeNone, (14, 16))],
        ),
    ]);
}

/// A generic method of an optional is untyped (the spec row `p.m()`), so its result gets no
/// report that it is not optional.
#[test]
fn a_generic_method_of_an_optional_should_be_untyped() {
    cell_rows(&[
        (
            "if let x = c.opt(c.v) { print(x ?? 0) }",
            &[(MayBeNone, (14, 16)), (MayBeNone, (14, 22))],
        ),
        (
            "print((c.opt(c.v) ?? 0) ?? 0)",
            &[(MayBeNone, (14, 12)), (MayBeNone, (14, 18))],
        ),
    ]);
}

/// A `??` with a member of an optional on either side is untyped (the spec row `??`), so the
/// `??` after it reports nothing.
#[test]
fn a_coalesce_over_a_poisoned_member_should_be_untyped() {
    cell_rows(&[
        ("print((Some(c.v) ?? 0) ?? 0)", &[(MayBeNone, (14, 17))]),
        ("print((c.n?.v ?? 0) ?? 0)", &[(MayBeNone, (14, 12))]),
    ]);
}

/// A `?.` after a member of an optional is untyped: the spec row index, member, method. So a
/// name bound to it is untyped. A `?.` on the optional itself keeps its type. A result that only
/// the member binds is untyped.
#[test]
fn a_safe_access_after_a_poisoned_member_should_be_untyped() {
    cell_rows(&[
        ("if let x = c.n?.v { x.nope() }", &[(MayBeNone, (14, 16))]),
        (
            "if let x = c?.opt(c.v) { print(x ?? 0) }",
            &[(MayBeNone, (14, 23))],
        ),
    ]);
}

/// A `let` of a member of an optional binds no type, also when the member's own type is optional
/// and a join carries it: the spec rows a binding with no declared type and `if`, `match` (a
/// branch).
#[test]
fn a_let_of_an_optional_poisoned_member_should_be_untyped() {
    cell_rows(&[
        ("let y = c.n\n    y.nope()", &[(MayBeNone, (14, 13))]),
        (
            "let y = if k { c.n } else { None }\n    y.nope()",
            &[(MayBeNone, (14, 20))],
        ),
    ]);
}

/// A member of an optional is untyped, so E0124 does not report a `match` on it: the spec row a
/// binding of a pattern.
#[test]
fn a_match_on_a_poisoned_member_should_not_report_e0124() {
    cell_rows(&[
        ("match c.ys { xs => print(xs) }", &[(MayBeNone, (14, 11))]),
        (
            "match id(c.ys) { xs => print(xs) }",
            &[(MayBeNone, (14, 14))],
        ),
    ]);
}

/// An index of a member of an optional is untyped: the spec row index, member, method. So the
/// `??` on the element reports nothing, and the index of the optional `oys` waits.
#[test]
fn an_index_of_a_poisoned_member_should_be_untyped() {
    rows(&[("print(c.oys[0][0] ?? 0)", &[(MayBeNone, (11, 11))])]);
}

/// The declarations of `call_rows`, then `fn f(c: C?)` whose body is the row's statements.
const CALL_DECLS: &str = "class C {\n    v: i64\n}\nclass Bx<T> {\n    v: T\n    fn put(self, x: i64) -> T { self.v }\n}\nclass B<T> {\n    v: T\n}\nenum E<T> {\n    V(T)\n}\ntype O = i64?\nfn id<T>(t: T) -> T { t }\nfn pair<T>(x: T, y: string) -> [T] { [x] }\nfn g<T>(n: i64, t: T) -> T { t }\nfn main() {}";

/// A row of `call_rows`: its name, its statements, and each diagnostic wanted. A line of a
/// diagnostic counts from the first line of the statements, which is 0.
type CallRow<'a> = (&'a str, &'a str, &'a [Wanted]);

/// Asserts the diagnostics of each row over `CALL_DECLS`.
fn call_rows(table: &[CallRow<'_>]) {
    let first = CALL_DECLS.lines().count() + 2;
    for &(row, body, wanted) in table {
        let src = format!("{CALL_DECLS}\nfn f(c: C?) {{\n    {body}\n}}");
        let want: Vec<Wanted> = wanted
            .iter()
            .map(|&(code, (line, col))| (code, (first + line, col)))
            .collect();
        codes_row(row, &src, &want);
    }
}

/// The receiver or a written type list can fix the result of a call. Such a result keeps its
/// type when an argument is a member of an optional, so the `??` on it is reported: the spec row
/// of a call. A result that only that argument binds is untyped, so its `??` and its mismatch
/// wait: `id(c.v)` and `E.V(c.v)`.
#[test]
fn a_call_result_should_be_untyped_only_when_a_failed_argument_binds_it() {
    call_rows(&[
        (
            "b1",
            "let b = Bx(v: 1)\n    print(b.put(c.v) ?? 0)",
            &[(CoalesceNotOptional, (1, 11)), (MayBeNone, (1, 17))],
        ),
        (
            "b2",
            "print(pair<i64>(c.v, \"s\") ?? [0])",
            &[(CoalesceNotOptional, (0, 11)), (MayBeNone, (0, 21))],
        ),
        (
            "b3",
            "print(id<i64>(n: c.v) ?? 0)",
            &[
                (CoalesceNotOptional, (0, 11)),
                (LabeledArgument, (0, 19)),
                (MayBeNone, (0, 22)),
            ],
        ),
        (
            "b4",
            "let y = g<i64>(c.v, 0)\n    print(y ?? 0)",
            &[(MayBeNone, (0, 20)), (CoalesceNotOptional, (1, 11))],
        ),
        (
            "b5",
            "let y = B<i64>(v: c.v)\n    print(y.nope())",
            &[(MayBeNone, (0, 23)), (NoMember, (1, 13))],
        ),
        (
            "b6",
            "let y = E<i64>.V(c.v)\n    let s: string = y",
            &[(MayBeNone, (0, 22)), (Mismatch, (1, 21))],
        ),
        ("g1", "print(id(c.v) ?? 0)", &[(MayBeNone, (0, 14))]),
        (
            "g2",
            "let s: string = O.Some(c.v)",
            &[(Mismatch, (0, 21)), (MayBeNone, (0, 28))],
        ),
        (
            "g3",
            "let s: string = Some<i64>(c.v)",
            &[(Mismatch, (0, 21)), (MayBeNone, (0, 31))],
        ),
        ("g4", "let s: string = E.V(c.v)", &[(MayBeNone, (0, 25))]),
        // The receiver's `T` is fixed, and the second E0062 stays: the argument `x` is a member
        // of an optional, as the receiver's is.
        (
            "b7",
            "print(Bx(v: c.v).put(x: c.v) ?? 0)",
            &[
                (MayBeNone, (0, 17)),
                (LabeledArgument, (0, 26)),
                (MayBeNone, (0, 29)),
            ],
        ),
    ]);
}

/// A call through an interface bound fixes its result by the requirement's own type. The result
/// keeps that type when an argument is a member of an optional.
#[test]
fn a_bound_method_whose_result_the_bound_fixes_should_keep_its_type() {
    codes(
        "interface Same {\n    fn same(self, x: i64) -> i64\n    fn back(self, x: i64) -> Self\n}\nclass C {\n    v: i64\n}\nfn f<T: Same>(t: T, c: C?) {\n    print(t.same(c.v) ?? 0)\n}\nfn g<T: Same>(t: T) {\n    let k = t.back(1)\n    let s: string = k\n}\nfn main() {}",
        &[
            (CoalesceNotOptional, (9, 11)),
            (MayBeNone, (9, 18)),
            (Mismatch, (13, 21)),
        ],
    );
}

/// A join of an `if let` with a branch that is a member of an optional is untyped (the spec row
/// `if`, `match`: a branch), so the `??` after it reports nothing.
#[test]
fn an_if_let_branch_of_a_poisoned_member_should_be_untyped() {
    codes(
        "class C {\n    v: i64\n}\nfn f(c: C?, k: i64?) {\n    print((if let x = k { c.v } else { 0 }) ?? 0)\n}\nfn main() {}",
        &[(MayBeNone, (5, 27))],
    );
}

/// A `match` on a member of an optional has an untyped subject, so an `Ok` pattern on it adds no
/// error: the spec row a binding of a pattern.
#[test]
fn a_constructor_pattern_on_a_poisoned_member_should_add_no_error() {
    codes(
        "class C {\n    v: i64\n}\nfn f(c: C?) {\n    match c.v {\n        Ok(x) => print(x)\n        _ => print(0)\n    }\n}\nfn main() {}",
        &[(MayBeNone, (5, 11))],
    );
}

/// A call whose result names a parameter that another argument binds, and whose one failed
/// argument is an unknown name, is untyped: the spec's call row lets no argument settle a
/// parameter in a call with a failed part. So the E0048 of its `??` waits. A call through a
/// function and through a bound each have the rule.
#[test]
fn a_call_whose_other_argument_binds_its_parameter_should_be_untyped() {
    codes(
        "interface Two {\n    fn two<U>(self, a: U, b: U) -> U\n}\nfn two<U>(a: U, b: U) -> U { a }\nfn f<T: Two>(t: T) {\n    print(t.two(1, nope) ?? 0)\n}\nfn g() {\n    print(two(1, nope) ?? 0)\n}\nfn main() {}",
        &[(BindingNotFound, (6, 20)), (BindingNotFound, (9, 18))],
    );
}

/// Each form of S14 as `let a = <form>`, then `print(a + true)`. The form decides whether the
/// poisoned value is typed, and not the source of the failure. An error that the rule adds is at
/// `true`, or at `a` when no `+` takes the type of `a`.
#[test]
#[expect(clippy::too_many_lines, reason = "one row for each row of S14")]
fn the_form_should_decide_the_type() {
    c_rows(&[
        (
            "if nothing() { 1 } else { 2 }",
            "let a = if nothing() { 1 } else { 2 } and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 16)), (Mismatch, (42, 15))],
        ),
        (
            "if p.name { 1 } else { 2 }",
            "let a = if p.name { 1 } else { 2 } and print(a + true)".into(),
            vec![(MayBeNone, (41, 16)), (Mismatch, (42, 15))],
        ),
        (
            "match nothing() { _ => 1 }",
            "let a = match nothing() { _ => 1 } and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 19)), (Mismatch, (42, 15))],
        ),
        (
            "{ nothing()\n 1 }",
            "let a = { nothing()\n 1 } and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 15)), (Mismatch, (43, 15))],
        ),
        (
            "h(nothing())",
            "let a = h(nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 15)), (Mismatch, (42, 15))],
        ),
        (
            "h(p.name)",
            "let a = h(p.name) and print(a + true)".into(),
            vec![(MayBeNone, (41, 15)), (Mismatch, (42, 15))],
        ),
        (
            "two(nothing(), 1)",
            "let a = two(nothing(), 1) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 17)), (Mismatch, (42, 15))],
        ),
        (
            "h()",
            "let a = h() and print(a + true)".into(),
            vec![(ArgumentCount, (41, 13)), (Mismatch, (42, 15))],
        ),
        (
            "keep(1)",
            "let a = keep(1) and print(a + true)".into(),
            vec![(ArgumentCount, (41, 13)), (Mismatch, (42, 15))],
        ),
        (
            "keep(1, 2, 3)",
            "let a = keep(1, 2, 3) and print(a + true)".into(),
            vec![(ArgumentCount, (41, 13)), (Mismatch, (42, 15))],
        ),
        (
            "keep(1, \"x\")",
            "let a = keep(1, \"x\") and print(a + true)".into(),
            vec![(Mismatch, (41, 21)), (Mismatch, (42, 15))],
        ),
        (
            "keep(nothing(), 1)",
            "let a = keep(nothing(), 1) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 18)), (Mismatch, (42, 15))],
        ),
        (
            "s.gm(nothing())",
            "let a = s.gm(nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 18)), (Mismatch, (42, 15))],
        ),
        (
            "id<i64>()",
            "let a = id<i64>() and print(a + true)".into(),
            vec![(ArgumentCount, (41, 13)), (Mismatch, (42, 15))],
        ),
        (
            "id<i64>(nothing())",
            "let a = id<i64>(nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 21)), (Mismatch, (42, 15))],
        ),
        (
            "keep<i64>(nothing(), 1)",
            "let a = keep<i64>(nothing(), 1) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 23)), (Mismatch, (42, 15))],
        ),
        (
            "Box<i64>(v: nothing())",
            "let a = Box<i64>(v: nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 25)), (Mismatch, (42, 11))],
        ),
        (
            "Some<i64>(nothing())",
            "let a = Some<i64>(nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 23)), (Mismatch, (42, 11))],
        ),
        (
            "bi.other(nothing())",
            "let a = bi.other(nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 22)), (Mismatch, (42, 15))],
        ),
        (
            "arr(nothing())",
            "let a = arr(nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 17)), (Mismatch, (42, 11))],
        ),
        (
            "keep<Missing>(1, 2)",
            "let a = keep<Missing>(1, 2) and print(a + true)".into(),
            vec![(UnknownType, (41, 18)), (Mismatch, (42, 15))],
        ),
        (
            "k + nothing()",
            "let a = k + nothing() and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 17)), (Mismatch, (42, 15))],
        ),
        (
            "k * nothing()",
            "let a = k * nothing() and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 17)), (Mismatch, (42, 15))],
        ),
        (
            "1.5 + nothing()",
            "let a = 1.5 + nothing() and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 19)), (Mismatch, (42, 15))],
        ),
        (
            "\"a\" + nothing()",
            "let a = \"a\" + nothing() and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 19)), (Mismatch, (42, 15))],
        ),
        (
            "[1; missing]",
            "let a = [1; missing] and print(a + true)".into(),
            vec![(BindingNotFound, (41, 17)), (Mismatch, (42, 11))],
        ),
        (
            "nothing() is i64",
            "let a = nothing() is i64 and print(a + true)".into(),
            vec![
                (FunctionNotFound, (41, 13)),
                (TypeAsValue, (41, 26)),
                (Mismatch, (42, 11)),
            ],
        ),
        (
            "match 1 { x if missing => 1\n _ => 2 }",
            "let a = match 1 { x if missing => 1\n _ => 2 } and print(a + true)".into(),
            vec![(BindingNotFound, (41, 28)), (Mismatch, (43, 15))],
        ),
        (
            "if c { return nothing() } else { 1 }",
            "let a = if c { return nothing() } else { 1 } and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 27)), (Mismatch, (42, 15))],
        ),
        (
            "if c { 1 } else { return missing }",
            "let a = if c { 1 } else { return missing } and print(a + true)".into(),
            vec![(BindingNotFound, (41, 38)), (Mismatch, (42, 15))],
        ),
        (
            "if c { 1 } else { panic(nothing()) }",
            "let a = if c { 1 } else { panic(nothing()) } and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 37)), (Mismatch, (42, 15))],
        ),
        (
            "o ?? return 0",
            "let a = o ?? return 0 and print(a + true)".into(),
            vec![(Mismatch, (42, 15))],
        ),
        (
            "Q(n: nothing())",
            "let a = Q(n: nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 18)), (Mismatch, (42, 11))],
        ),
        (
            "Q(n: p.name)",
            "let a = Q(n: p.name) and print(a + true)".into(),
            vec![(MayBeNone, (41, 18)), (Mismatch, (42, 11))],
        ),
        (
            "E.V(nothing())",
            "let a = E.V(nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 17)), (Mismatch, (42, 11))],
        ),
        (
            "e.k(nothing())",
            "let a = e.k(nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 17)), (Mismatch, (42, 15))],
        ),
        (
            "nothing() < 1",
            "let a = nothing() < 1 and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 13)), (Mismatch, (42, 11))],
        ),
        (
            "1 < nothing()",
            "let a = 1 < nothing() and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 17)), (Mismatch, (42, 11))],
        ),
        (
            "p.name < 1",
            "let a = p.name < 1 and print(a + true)".into(),
            vec![(MayBeNone, (41, 13)), (Mismatch, (42, 11))],
        ),
        (
            "nothing() == 1",
            "let a = nothing() == 1 and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 13)), (Mismatch, (42, 11))],
        ),
        (
            "!nothing()",
            "let a = !nothing() and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 14)), (Mismatch, (42, 11))],
        ),
        (
            "nothing() && true",
            "let a = nothing() && true and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 13)), (Mismatch, (42, 11))],
        ),
        (
            "true && nothing()",
            "let a = true && nothing() and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 21)), (Mismatch, (42, 11))],
        ),
        (
            "xs[0..missing]",
            "let a = xs[0..missing] and print(a + true)".into(),
            vec![(BindingNotFound, (41, 19)), (Mismatch, (42, 11))],
        ),
        (
            "i64(nothing())",
            "let a = i64(nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 17)), (Mismatch, (42, 15))],
        ),
        (
            "string(nothing())",
            "let a = string(nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 20)), (Mismatch, (42, 15))],
        ),
        (
            "\"{missing}\"",
            "let a = \"{missing}\" and print(a + true)".into(),
            vec![(BindingNotFound, (41, 15)), (Mismatch, (42, 15))],
        ),
        (
            "{ 1\n nothing() }",
            "let a = { 1\n nothing() } and print(a + true)".into(),
            vec![(FunctionNotFound, (42, 6))],
        ),
        (
            "if c { 1 } else { nothing() }",
            "let a = if c { 1 } else { nothing() } and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 31))],
        ),
        (
            "if nothing() { 1 } else { nothing() }",
            "let a = if nothing() { 1 } else { nothing() } and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 16)), (FunctionNotFound, (41, 39))],
        ),
        (
            "match c { true => 1\n false => nothing() }",
            "let a = match c { true => 1\n false => nothing() } and print(a + true)".into(),
            vec![(FunctionNotFound, (42, 15))],
        ),
        (
            "[1, nothing()]",
            "let a = [1, nothing()] and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 17))],
        ),
        (
            "[nothing(), 1]",
            "let a = [nothing(), 1] and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 14))],
        ),
        (
            "[nothing()]",
            "let a = [nothing()] and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 14))],
        ),
        (
            "[p.name]",
            "let a = [p.name] and print(a + true)".into(),
            vec![(MayBeNone, (41, 14))],
        ),
        (
            "Some(nothing())",
            "let a = Some(nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 18))],
        ),
        (
            "Ok(nothing())",
            "let a = Ok(nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 16))],
        ),
        (
            "Err(nothing())",
            "let a = Err(nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 17))],
        ),
        (
            "dbg(nothing())",
            "let a = dbg(nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 17))],
        ),
        (
            "id(nothing())",
            "let a = id(nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 16))],
        ),
        (
            "s.gn(nothing())",
            "let a = s.gn(nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 18))],
        ),
        (
            "G.W(nothing())",
            "let a = G.W(nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 17))],
        ),
        (
            "Box(v: nothing())",
            "let a = Box(v: nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 20))],
        ),
        (
            "id()",
            "let a = id() and print(a + true)".into(),
            vec![(ArgumentCount, (41, 13))],
        ),
        (
            "pick(1)",
            "let a = pick(1) and print(a + true)".into(),
            vec![(ArgumentCount, (41, 13))],
        ),
        (
            "pick(1, 2, 3)",
            "let a = pick(1, 2, 3) and print(a + true)".into(),
            vec![(ArgumentCount, (41, 13))],
        ),
        (
            "both(1)",
            "let a = both(1) and print(a + true)".into(),
            vec![(ArgumentCount, (41, 13))],
        ),
        (
            "Box()",
            "let a = Box() and print(a + true)".into(),
            vec![(MissingField, (41, 13))],
        ),
        (
            "Box(v: 1, w: 2)",
            "let a = Box(v: 1, w: 2) and print(a + true)".into(),
            vec![(TooManyArguments, (41, 23))],
        ),
        (
            "[nothing(); 3]",
            "let a = [nothing(); 3] and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 14))],
        ),
        (
            "nothing() + 1",
            "let a = nothing() + 1 and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 13))],
        ),
        (
            "nothing() * k",
            "let a = nothing() * k and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 13))],
        ),
        (
            "1 + nothing()",
            "let a = 1 + nothing() and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 17))],
        ),
        (
            "-1 + nothing()",
            "let a = -1 + nothing() and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 18))],
        ),
        (
            "(1 + 2) + nothing()",
            "let a = (1 + 2) + nothing() and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 23))],
        ),
        (
            "{ 1 } + nothing()",
            "let a = { 1 } + nothing() and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 21))],
        ),
        (
            "if c { 1 } else { 2 } + nothing()",
            "let a = if c { 1 } else { 2 } + nothing() and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 37))],
        ),
        (
            "id(1) + nothing()",
            "let a = id(1) + nothing() and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 21))],
        ),
        (
            "pick(1, nothing())",
            "let a = pick(1, nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 21))],
        ),
        (
            "both(k, nothing())",
            "let a = both(k, nothing()) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 21))],
        ),
        (
            "id(h())",
            "let a = id(h()) and print(a + true)".into(),
            vec![(ArgumentCount, (41, 16))],
        ),
        (
            "both(1, \"x\")",
            "let a = both(1, \"x\") and print(a + true)".into(),
            vec![(Mismatch, (41, 18))],
        ),
        (
            "bd(Rock(), 1)",
            "let a = bd(Rock(), 1) and print(a + true)".into(),
            vec![(NotConform, (41, 16))],
        ),
        (
            "id<Missing>(1)",
            "let a = id<Missing>(1) and print(a + true)".into(),
            vec![(UnknownType, (41, 16))],
        ),
        (
            "true + nothing()",
            "let a = true + nothing() and print(a + true)".into(),
            vec![(Mismatch, (41, 13)), (FunctionNotFound, (41, 20))],
        ),
        (
            "o + nothing()",
            "let a = o + nothing() and print(a + true)".into(),
            vec![(Mismatch, (41, 13)), (FunctionNotFound, (41, 17))],
        ),
        (
            "o ?? return missing",
            "let a = o ?? return missing and print(a + true)".into(),
            vec![(BindingNotFound, (41, 25))],
        ),
        (
            "-nothing()",
            "let a = -nothing() and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 14))],
        ),
        (
            "xs[missing]",
            "let a = xs[missing] and print(a + true)".into(),
            vec![(BindingNotFound, (41, 16))],
        ),
        (
            "missing[0]",
            "let a = missing[0] and print(a + true)".into(),
            vec![(BindingNotFound, (41, 13))],
        ),
        (
            "nothing().x",
            "let a = nothing().x and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 13))],
        ),
        (
            "nothing().m()",
            "let a = nothing().m() and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 13))],
        ),
        (
            "nothing() ?? 1",
            "let a = nothing() ?? 1 and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 13))],
        ),
        (
            "o ?? nothing()",
            "let a = o ?? nothing() and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 18))],
        ),
        (
            "loop { break nothing() }",
            "let a = loop { break nothing() } and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 26))],
        ),
        (
            "loop { if c { break 1 }\n break nothing() }",
            "let a = loop { if c { break 1 }\n break nothing() } and print(a + true)".into(),
            vec![(FunctionNotFound, (42, 12))],
        ),
        (
            "p.name",
            "let a = p.name and print(a + true)".into(),
            vec![(MayBeNone, (41, 13))],
        ),
        (
            "p.m()",
            "let a = p.m() and print(a + true)".into(),
            vec![(MayBeNone, (41, 13))],
        ),
        (
            "p.u()",
            "let a = p.u() and print(a + true)".into(),
            vec![(MayBeNone, (41, 13))],
        ),
    ]);
}

/// The statements `stmts` (each part between ` and ` is one line) in the body of `f`, from line
/// 2. `f` has an array `xs` and an optional `o`.
fn in_f(stmts: &str) -> String {
    let body = stmts
        .split(" and ")
        .fold(String::new(), |body, line| body + "    " + line + "\n");
    format!("fn f(xs: [i64], o: i64?) -> i64 {{\n{body}    return 1\n}}\nfn main() {{}}\n")
}

/// A declared type, a range and the `else` block of `let ... else` state the type apart from
/// the failed part. So the true errors after it stay (S16).
#[test]
fn a_type_known_for_certain_should_keep_its_true_errors() {
    error_rows(&[
        (
            "declared",
            in_f("let x: i64 = nothing() and print(x + true)"),
            vec![(FunctionNotFound, (2, 18)), (Mismatch, (3, 15))],
        ),
        (
            "no type",
            in_f("let x = nothing() and print(x + true)"),
            vec![(FunctionNotFound, (2, 13))],
        ),
        (
            "sibling",
            in_f("print(missing + other)"),
            vec![(BindingNotFound, (2, 11)), (BindingNotFound, (2, 21))],
        ),
        (
            "one edit",
            in_f("let n = true and print(n + 1) and print(n * 2)"),
            vec![(Mismatch, (3, 11)), (Mismatch, (4, 11))],
        ),
        (
            "declared ??",
            in_f("let x: i64 = nothing() and print(x ?? 1)"),
            vec![(FunctionNotFound, (2, 18)), (CoalesceNotOptional, (3, 11))],
        ),
        (
            "for range",
            in_f("for x in 0..missing { print(x + true) }"),
            vec![(BindingNotFound, (2, 17)), (Mismatch, (2, 37))],
        ),
        (
            "for range index",
            in_f("for x in xs[0..missing] { print(x + true) }"),
            vec![(BindingNotFound, (2, 20)), (Mismatch, (2, 41))],
        ),
        (
            "for failed",
            in_f("for x in missing { print(x + true) }"),
            vec![(BindingNotFound, (2, 14))],
        ),
        (
            "let else failed",
            in_f("let Some(y) = missing else { return 1 } and print(y + true)"),
            vec![(BindingNotFound, (2, 19))],
        ),
        (
            "let else block",
            in_f("let Some(y) = o else { return missing } and print(y + true)"),
            vec![(BindingNotFound, (2, 35)), (Mismatch, (3, 15))],
        ),
    ]);
}

/// `mismatch` and `jumped` keep their jobs: the operator's own failure is the one error (S17).
#[test]
fn an_operator_failure_should_stay_one_report() {
    error_rows(&[
        (
            "mismatch",
            in_f("let n: i64 = None && true"),
            vec![(Mismatch, (2, 18))],
        ),
        (
            "jumped",
            in_f("let y: i64 = { break }"),
            vec![(JumpOutsideLoop, (2, 20))],
        ),
    ]);
}

/// A generic call that cannot meet its context keeps its message: `found` and `Misfit` give the
/// E0032 at the `return` and each E0050 at the operator (S18).
#[test]
fn a_failed_generic_call_should_keep_its_message() {
    error_rows(&[
        (
            "s18",
            "class Two<A, B> {\n    a: A\n    b: B\n}\nfn mk2<X>() -> Two<X, bool> { panic(\"e\") }\nfn id<V>(v: V) -> V { v }\nfn r() -> i64 { return id(mk2()) }\nfn main() {\n    let z = id(mk2()) + id(mk2())\n}\n".into(),
            vec![(Mismatch, (7, 27)), (CannotInfer, (9, 16)), (CannotInfer, (9, 28))],
        ),
    ]);
}

/// A closed result is typed only when each parameter that it names is bound: a written type arg
/// binds it, and an argument beside a failed part does not. A type arg that its bound rejects
/// binds nothing, and a result that names no parameter stays typed past it.
#[test]
fn a_closed_result_should_need_each_parameter_bound() {
    c_rows(&[
        (
            "closed by arg",
            "let a = pick<i64>(nothing(), 1) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 23)), (Mismatch, (42, 15))],
        ),
        (
            "open",
            "let a = pick(nothing(), 1) and print(a + true)".into(),
            vec![(FunctionNotFound, (41, 18))],
        ),
        (
            "rejected bound",
            "let a = bd<Rock>(Rock(), 1) and print(a + true)".into(),
            vec![(NotConform, (41, 16))],
        ),
        (
            "closed past a bound",
            "let a = kb<Rock>(Rock()) and print(a + true)".into(),
            vec![(NotConform, (41, 16)), (Mismatch, (42, 15))],
        ),
    ]);
}

/// `h()` is a typed poisoned `i64`. As an argument of a call it is a failed part, so the call is
/// untyped. In each other form it reads as an `i64`, so the true errors after it stay. Beside it,
/// the other arguments and the bounds are still checked against its type. A typed poisoned
/// receiver of a method reads as its type too.
#[test]
#[expect(clippy::too_many_lines, reason = "one row for each form of the table")]
fn a_typed_poisoned_part_should_fail_only_a_call() {
    c_rows(&[
        (
            "id",
            "let a = id(h()) and print(a + true)".into(),
            vec![(ArgumentCount, (41, 16))],
        ),
        (
            "some",
            "let a = Some(h()) and print(a + true)".into(),
            vec![(ArgumentCount, (41, 18))],
        ),
        (
            "dbg",
            "let a = dbg(h()) and print(a + true)".into(),
            vec![(ArgumentCount, (41, 17))],
        ),
        (
            "pick",
            "let a = pick(h(), 1) and print(a + true)".into(),
            vec![(ArgumentCount, (41, 18))],
        ),
        // A typed poisoned receiver reads as its type, so the argument binds the result.
        (
            "typed receiver",
            "let a = { nope; s }.gn(1) and print(a + true)".into(),
            vec![(BindingNotFound, (41, 15)), (Mismatch, (42, 15))],
        ),
        (
            "built receiver",
            "let a = P(name: 1, v: 1).gn(1) and print(a + true)".into(),
            vec![(Mismatch, (41, 21)), (Mismatch, (42, 15))],
        ),
        (
            "receiver and argument",
            "let a = { nope; s }.gn(h()) and print(a + true)".into(),
            vec![(BindingNotFound, (41, 15)), (ArgumentCount, (41, 28))],
        ),
        (
            "plus k",
            "let a = h() + k and print(a + true)".into(),
            vec![(ArgumentCount, (41, 13)), (Mismatch, (42, 15))],
        ),
        (
            "plus 1",
            "let a = h() + 1 and print(a + true)".into(),
            vec![(ArgumentCount, (41, 13)), (Mismatch, (42, 15))],
        ),
        (
            "k plus",
            "let a = k + h() and print(a + true)".into(),
            vec![(ArgumentCount, (41, 17)), (Mismatch, (42, 15))],
        ),
        (
            "1 plus",
            "let a = 1 + h() and print(a + true)".into(),
            vec![(ArgumentCount, (41, 17)), (Mismatch, (42, 15))],
        ),
        (
            "conv plus",
            "let a = i64(nope) + k and print(a + true)".into(),
            vec![(BindingNotFound, (41, 17)), (Mismatch, (42, 15))],
        ),
        (
            "block plus",
            "let a = { nope\n 5 } + 1 and print(a + true)".into(),
            vec![(BindingNotFound, (41, 15)), (Mismatch, (43, 15))],
        ),
        (
            "neg",
            "let a = -h() and print(a + true)".into(),
            vec![(ArgumentCount, (41, 14)), (Mismatch, (42, 15))],
        ),
        (
            "neg conv",
            "let a = -i64(nope) and print(a + true)".into(),
            vec![(BindingNotFound, (41, 18)), (Mismatch, (42, 15))],
        ),
        (
            "index",
            "let a = xs[h()] and print(a + true)".into(),
            vec![(ArgumentCount, (41, 16)), (Mismatch, (42, 15))],
        ),
        (
            "less",
            "let a = h() < 1 and print(a + true)".into(),
            vec![(ArgumentCount, (41, 13)), (Mismatch, (42, 11))],
        ),
        (
            "array",
            "let a = [h()] and print(a + true)".into(),
            vec![(ArgumentCount, (41, 14)), (Mismatch, (42, 11))],
        ),
        (
            "repeat",
            "let a = [h(); 3] and print(a + true)".into(),
            vec![(ArgumentCount, (41, 14)), (Mismatch, (42, 11))],
        ),
        (
            "block",
            "let a = { h() } and print(a + true)".into(),
            vec![(ArgumentCount, (41, 15)), (Mismatch, (42, 15))],
        ),
        (
            "if",
            "let a = if c { 1 } else { h() } and print(a + true)".into(),
            vec![(ArgumentCount, (41, 31)), (Mismatch, (42, 15))],
        ),
        (
            "match",
            "let a = match c { true => 1\n false => h() } and print(a + true)".into(),
            vec![(ArgumentCount, (42, 15)), (Mismatch, (43, 15))],
        ),
        (
            "coalesce",
            "let a = o ?? h() and print(a + true)".into(),
            vec![(ArgumentCount, (41, 18)), (Mismatch, (42, 15))],
        ),
        (
            "interp",
            "let a = \"{h()}\" and print(a + true)".into(),
            vec![(ArgumentCount, (41, 15)), (Mismatch, (42, 15))],
        ),
        (
            "i64 conv",
            "let a = i64(h()) and print(a + true)".into(),
            vec![(ArgumentCount, (41, 17)), (AlreadyConverted, (41, 17))],
        ),
        (
            "string conv",
            "let a = string(h()) and print(a + true)".into(),
            vec![(ArgumentCount, (41, 20)), (Mismatch, (41, 20))],
        ),
        (
            "both",
            "let a = both(h(), true)".into(),
            vec![(ArgumentCount, (41, 18)), (Mismatch, (41, 23))],
        ),
        (
            "bd",
            "let a = bd(h(), 1)".into(),
            vec![(ArgumentCount, (41, 16)), (NotConform, (41, 16))],
        ),
        (
            "ok",
            "let a = Ok(h()) and print(a + true)".into(),
            vec![(CannotInfer, (41, 13)), (ArgumentCount, (41, 16))],
        ),
        (
            "err",
            "let a = Err(h()) and print(a + true)".into(),
            vec![(CannotInfer, (41, 13)), (ArgumentCount, (41, 17))],
        ),
    ]);
    // A typed poisoned receiver of an interface method reads as its type too.
    error_rows(&[(
        "typed bound receiver",
        "interface D {\n    fn dg<U>(self, a: U) -> U\n}\nfn f<T: D>(t: T) {\n    let a = { nope\n        t }.dg(1)\n    print(a + true)\n}\nfn main() {}\n".into(),
        vec![(BindingNotFound, (5, 15)), (Mismatch, (7, 15))],
    )]);
}

/// GIVEN an enum `E` whose only conformance block names an unknown interface, so that `E` has
///   `Eq` only through that broken block
/// WHEN `==` compares two values of `E`, both checked or one a typed poisoned call
/// THEN the `==` is untyped: a later `+` adds no error, and only the earlier errors stay
#[test]
fn equality_on_a_type_with_eq_only_from_a_broken_block_should_be_untyped() {
    for (row, src, wanted) in [
        (
            "two variants",
            "enum E {\n    A\n}\nextend E: Nope {}\nfn main() {\n    let a = E.A == E.A\n    \
             print(a + 1)\n}\n",
            vec![(UnknownType, (4, 11))],
        ),
        (
            "a typed poisoned side",
            "enum E {\n    A\n}\nextend E: Nope {}\nfn mk(x: i64) -> E {\n    E.A\n}\n\
             fn main() {\n    let a = E.A == mk()\n    print(a + 1)\n}\n",
            vec![(UnknownType, (4, 11)), (ArgumentCount, (9, 20))],
        ),
    ] {
        codes_row(row, src, &wanted);
    }
}
