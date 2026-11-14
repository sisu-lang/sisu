//! `mut self` on a struct or an enum method: the declaration, the receiver of a call, and a bare
//! `self` as a place.

use super::*;
use Code::{
    AssignToExpression, AssignToImmutable, AssignToSelf, CannotInfer, ConformanceBody, Expected,
    MayBeNone, Mismatch, MutArgumentNotPlace, MutMarkerMissing, MutPlaceTwice, MutSelf,
    NeverReassigned, SelfOutsideMethod, UnusedMember,
};

/// `Point` with the `mut self` method `move_by`, in eight lines.
const POINT: &str = "struct Point {\n    x: i64\n    y: i64\n    fn move_by(mut self, dx: i64, dy: i64) {\n        self.x += dx\n        self.y += dy\n    }\n}\n";

/// The line and the column of the first `needle` on line `line` of `src`.
fn on(src: &str, line: usize, needle: &str) -> (usize, usize) {
    let text = src.lines().nth(line - 1).expect("the line should exist");
    let col = text.find(needle).expect("the line should hold the needle");
    (line, col + 1)
}

/// The code and the position of each diagnostic of `src` with `extra` after it, which starts
/// before `extra`. `extra` goes last, so that it moves no position of `src`.
fn codes_without(src: &str, extra: &str) -> Vec<Wanted> {
    let both = format!("{src}{extra}");
    let (_, ds) = checked(&both);
    (ds.iter())
        .filter(|d| d.span().start() < src.len())
        .map(|d| (d.code(), at(&both, d.span())))
        .collect()
}

/// The code and the position of each diagnostic of `src`.
fn all_codes(src: &str) -> Vec<Wanted> {
    let (_, ds) = checked(src);
    ds.iter().map(|d| (d.code(), at(src, d.span()))).collect()
}

/// GIVEN `Point` with `move_by`. WHEN `main` runs `let mut p = Point(x: 1, y: 2)`,
/// `p.move_by(3, 4)` and prints `p`. THEN `sisu check` reports no W0002 and no W0014.
#[test]
fn mut_self_call_should_report_no_unused_warning() {
    judge_unused();
    let src = format!(
        "{POINT}fn main() {{\n    let mut p = Point(x: 1, y: 2)\n    p.move_by(3, 4)\n    print(p)\n}}\n"
    );
    codes(&src, &[]);
}

/// GIVEN `Point` with `move_by`, `struct L { p: Point; ps: [Point] }`, `let ps = [...]`,
/// `fn mk() -> L`, `class D { p: Point; mut q: Point }`, `fn mk_d() -> D` and
/// `let mut op: Point? = ...`. WHEN the program calls `move_by` on a receiver without a root that
/// can change. THEN `sisu check` reports the row's code at the row's position.
#[test]
#[expect(clippy::too_many_lines, reason = "a table of rows")]
fn receiver_without_a_changeable_root_should_be_rejected() {
    // Line 9 holds `L`; `main` is at line 13, and its row starts at line 16.
    let program = |items: &str, body: &str| {
        format!(
            "{POINT}struct L {{ p: Point; ps: [Point] }}\nclass D {{ p: Point; mut q: Point }}\n\
             fn mk() -> L {{ L(p: Point(x: 1, y: 2), ps: []) }}\n\
             fn mk_d() -> D {{ D(p: Point(x: 1, y: 2), q: Point(x: 1, y: 2)) }}\n\
             fn main() {{\n    let ps = [Point(x: 1, y: 2)]\n    let mut op: Point? = Some(Point(x: 1, y: 2))\n    {body}\n}}\n{items}"
        )
    };
    // The items go after `main`, at line 18.
    let item = |items: &str| program(items, "print(ps)");
    let rows: Vec<(&str, String, Vec<Wanted>)> = vec![
        (
            "a `let`",
            program("", "let p = Point(x: 1, y: 2); p.move_by(1, 1)"),
            vec![(AssignToImmutable, (16, 32))],
        ),
        (
            "a parameter",
            item("fn f(p: Point) { p.move_by(1, 1) }"),
            vec![(AssignToImmutable, (18, 18))],
        ),
        (
            "plain `self`",
            item("extend Point { fn f(self) { self.move_by(1, 1) } }"),
            vec![(AssignToImmutable, (18, 29))],
        ),
        (
            "a field of plain `self`",
            item("extend L { fn f(self) { self.p.move_by(1, 1) } }"),
            vec![(AssignToImmutable, (18, 25))],
        ),
        (
            "a `for` binding",
            program("", "for p in ps { p.move_by(1, 1) }"),
            vec![(AssignToImmutable, (16, 19))],
        ),
        (
            "an `if let` binding",
            program("", "if let Some(p) = op { p.move_by(1, 1) }"),
            vec![(AssignToImmutable, (16, 27))],
        ),
        (
            "a `while let` binding",
            program("", "while let Some(p) = op { p.move_by(1, 1); break }"),
            vec![(AssignToImmutable, (16, 30))],
        ),
        (
            "a `match` binding",
            program(
                "",
                "match op {\n        Some(p) => p.move_by(1, 1)\n        None => {}\n    }",
            ),
            vec![(AssignToImmutable, (17, 20))],
        ),
        (
            "a `let ... else` binding",
            program("", "let Some(p) = op else { return }\n    p.move_by(1, 1)"),
            vec![(AssignToImmutable, (17, 5))],
        ),
        (
            "a class field without `mut`",
            program("", "let d = mk_d(); d.p.move_by(1, 1)"),
            vec![(AssignToImmutable, (16, 21))],
        ),
        (
            "a temporary",
            program("", "Point(x: 1, y: 2).move_by(1, 1)"),
            vec![(AssignToExpression, (16, 5))],
        ),
        (
            "a part of a temporary",
            program("", "mk().p.move_by(1, 1)"),
            vec![(AssignToExpression, (16, 5))],
        ),
        (
            "a `mut` field of an object",
            program("", "mk_d().q.move_by(1, 1)"),
            vec![],
        ),
        (
            "no type on `mut self`",
            item("extend Point { fn f(mut self: Point) {} }"),
            vec![(Expected, (18, 29))],
        ),
    ];
    error_rows(&rows);
    // The `let` row has the fix `let mut`, and the class field row the help "mark the field
    // `mut`".
    let src = program("", "let p = Point(x: 1, y: 2); p.move_by(1, 1)");
    let (_, ds) = checked(&src);
    let d = ds
        .iter()
        .find(|d| d.code() == AssignToImmutable)
        .expect("the `let` row should be E0001");
    assert_eq!(
        fixes(&src, d),
        [(true, vec![(16, 9, "", "mut ".to_string())])],
        "{d:?}"
    );
    let src = program("", "let d = mk_d(); d.p.move_by(1, 1)");
    let (_, ds) = checked(&src);
    let d = ds
        .iter()
        .find(|d| d.code() == AssignToImmutable)
        .expect("the class field row should be E0001");
    assert!(d.help().is_some() && d.fixes().is_empty(), "{d:?}");
}

/// GIVEN `Point` with `move_by`, `enum E { A; B; fn flip(mut self) }`, and an optional value.
/// WHEN the program runs a `mut self` call whose receiver's path holds a `?.`. THEN `sisu check`
/// reports E0062 at the first `?.`, and no other error.
#[test]
fn mut_self_call_through_an_optional_value_should_be_e0062() {
    let program = |decl: &str, call: &str| {
        format!(
            "{POINT}enum E {{\n    A\n    B\n    fn flip(mut self) {{ self = E.B }}\n}}\n\
             struct L {{ p: Point }}\nfn main() {{\n    {decl}\n    {call}\n}}\n"
        )
    };
    let rows = [
        (
            "a struct value",
            program(
                "let mut o: Point? = Some(Point(x: 1, y: 2))",
                "o?.move_by(1, 1)",
            ),
        ),
        (
            "an enum value",
            program("let mut e: E? = Some(E.A)", "e?.flip()"),
        ),
        (
            "a field after the `?.`",
            program(
                "let mut o: L? = Some(L(p: Point(x: 1, y: 2)))",
                "o?.p.move_by(1, 1)",
            ),
        ),
    ];
    for (row, src) in &rows {
        let (_, ds) = checked(src);
        assert_eq!(errors(src, &ds), [(MayBeNone, (17, 6))], "row `{row}`");
    }
    // A write through `?.` of a field of a `mut self`.
    let src = "struct In { n: i64 }\nfn bump(mut x: i64) { x = 1 }\n\
               struct P { n: i64; xs: [i64]; i: In; o: In? }\n\
               extend P {\n    fn m(mut self) { self.o?.n = 2 }\n}\nfn main() {}";
    let (_, ds) = checked(src);
    assert_eq!(
        errors(src, &ds),
        [(MayBeNone, (5, 28))],
        "a write through `?.`"
    );
}

/// GIVEN `struct A { n: i64; fn len(mut self) }`, after a program whose plain methods share the
/// name `len`. WHEN the program is checked. THEN each row has the codes of the same program
/// without `A`: the receiver of a plain method keeps its rules.
#[test]
fn plain_method_with_a_mut_self_name_should_keep_its_receiver_rules() {
    judge_unused();
    let a = "struct A { n: i64; fn len(mut self) { self.n = 1 } }\n";
    let rows = [
        (
            "an array that is never changed",
            "fn main() {\n    let mut xs = [1]\n    print(xs.len())\n}\n",
            vec![(NeverReassigned, (2, 13))],
        ),
        (
            "an array in a class field",
            "class C { items: [i64] }\nfn main() {\n    let c = C(items: [1])\n    print(c.items.len())\n}\n",
            vec![],
        ),
        (
            "a `for` binding",
            "fn main() {\n    for x in [[1]] {\n        print(x.len())\n    }\n}\n",
            vec![],
        ),
        (
            "a temporary",
            "fn mk() -> [i64] { [1] }\nfn main() {\n    print(mk().len())\n}\n",
            vec![],
        ),
        (
            "a `?.` receiver",
            "fn main() {\n    let o: [i64]? = Some([1])\n    print(o?.len())\n}\n",
            vec![],
        ),
        (
            "a struct field that nothing else reads",
            "struct P { x: i64; fn len(self) -> i64 { self.x } }\nstruct S { p: P }\nfn main() {\n    let s = S(p: P(x: 1))\n    print(s.p.len())\n}\n",
            vec![],
        ),
    ];
    for (row, src, wanted) in &rows {
        assert_eq!(all_codes(src), *wanted, "row `{row}` without `A`");
        assert_eq!(codes_without(src, a), *wanted, "row `{row}` with `A`");
    }
    // The shortcut's named change: a plain generic method that shares the name loses the
    // context of the outer call.
    let src = "struct B { n: i64; fn len<U>(self) -> U? { None } }\nfn id<T>(t: T) -> T { t }\n\
               fn main() {\n    let b = B(n: 1)\n    let x: i64? = id(b.len())\n    print(x)\n}\n";
    let (_, ds) = checked(src);
    assert_eq!(errors(src, &ds), [], "without `A`");
    let both = format!("{src}{a}");
    let (_, ds) = checked(&both);
    assert_eq!(
        errors(&both, &ds),
        [(CannotInfer, on(src, 5, "len"))],
        "with `A`"
    );
}

/// GIVEN a method body with `mut self` in a struct, an enum, an `extend` of each, or a class.
/// WHEN the program is checked. THEN `mut self` has no error where it has meaning, and each
/// other form keeps its code.
#[test]
fn mut_self_should_parse_where_it_has_meaning() {
    let assign = |decl: &str, holder: &str, value: &str, recv: &str| {
        format!(
            "{decl}\nextend {holder} {{\n    fn f({recv}) {{ self = {value} }}\n}}\nfn main() {{}}"
        )
    };
    let rows: Vec<(&str, String, Vec<Wanted>)> = vec![
        (
            "a struct body",
            "struct P {\n    x: i64\n    fn f(mut self) { self.x = 1 }\n}\nfn main() {}"
                .to_string(),
            vec![],
        ),
        (
            "an enum body",
            "enum E {\n    A\n    B\n    fn f(mut self) { self = E.B }\n}\nfn main() {}"
                .to_string(),
            vec![],
        ),
        (
            "an `extend` of a struct",
            assign("struct P { x: i64 }", "P", "P(x: 1)", "mut self"),
            vec![],
        ),
        (
            "an `extend` of an enum",
            assign("enum E { A; B }", "E", "E.B", "mut self"),
            vec![],
        ),
        (
            "a class body",
            "class C {\n    fn f(mut self) {}\n}\nfn main() {}".to_string(),
            vec![(MutSelf, (2, 10))],
        ),
        (
            "a type on `mut self` in a struct body",
            "struct P {\n    x: i64\n    fn f(mut self: P) {}\n}\nfn main() {}".to_string(),
            vec![(Expected, (3, 18))],
        ),
        (
            "a type on `mut self` in an `extend` of a struct",
            "struct P { x: i64 }\nextend P {\n    fn f(mut self: P) {}\n}\nfn main() {}"
                .to_string(),
            vec![(Expected, (3, 18))],
        ),
        (
            "`self = e` in a plain struct body method",
            "struct T {\n    n: i64\n    fn f(self) { self = T(n: 1) }\n}\nfn main() {}"
                .to_string(),
            vec![(AssignToSelf, (3, 18))],
        ),
        (
            "`self = e` in a plain enum body method",
            "enum E {\n    A\n    B\n    fn f(self) { self = E.B }\n}\nfn main() {}".to_string(),
            vec![(AssignToSelf, (4, 18))],
        ),
        (
            "`self = e` in a plain method of an `extend` of a struct",
            assign("struct P { x: i64 }", "P", "P(x: 1)", "self"),
            vec![(AssignToSelf, (3, 18))],
        ),
        (
            "`self = e` in a plain method of an `extend` of an enum",
            assign("enum E { A; B }", "E", "E.B", "self"),
            vec![(AssignToSelf, (3, 18))],
        ),
        (
            "`self = e` in a `mut self` method of an `extend` of a class",
            assign("class C {\n    mut n: i64\n}", "C", "C(n: 1)", "mut self"),
            vec![(MutSelf, (5, 10)), (AssignToSelf, (5, 22))],
        ),
        (
            "`bump(mut self)` in a `mut self` method of an `extend` of a class",
            "class C {\n    mut n: i64\n}\nextend C {\n    fn f(mut self) { bump(mut self) }\n}\n\
             fn bump(mut c: C) {}\nfn main() {}"
                .to_string(),
            vec![(MutSelf, (5, 10)), (MutArgumentNotPlace, (5, 31))],
        ),
        (
            "`self[0] = 1` in a class body",
            "class C {\n    n: i64\n    fn f(self) { self[0] = 1 }\n}\nfn main() {}".to_string(),
            vec![(AssignToExpression, (3, 18))],
        ),
        (
            "`self[0] = 1` in a struct body",
            "struct T {\n    n: i64\n    fn f(self) { self[0] = 1 }\n}\nfn main() {}".to_string(),
            vec![(AssignToExpression, (3, 18))],
        ),
        (
            "`self[0] = 1` in a free function",
            "fn main() {\n    self[0] = 1\n}".to_string(),
            vec![(AssignToExpression, (2, 5))],
        ),
    ];
    error_rows(&rows);
}

/// GIVEN `struct A { n: i64; fn bump(mut self) }`, after a program that uses a bare `self`.
/// WHEN the program is checked. THEN a bare `self` is a place in a struct or an enum method only,
/// and each row has its codes.
#[test]
fn bare_self_should_be_a_place_only_in_a_struct_or_enum_method() {
    let a = "struct A { n: i64; fn bump(mut self) { self.n = 1 } }\n";
    let rows: Vec<(&str, String, Vec<Wanted>)> = vec![
        (
            "`self.push` in a free function",
            "fn main() {\n    self.push(1)\n}\n".to_string(),
            vec![(SelfOutsideMethod, (2, 5))],
        ),
        (
            "`self.push` in a function with an array parameter",
            "fn f(xs: [i64]) {\n    self.push(1)\n}\nfn main() {}\n".to_string(),
            vec![(SelfOutsideMethod, (2, 5))],
        ),
        (
            "a struct's own plain `push` on `self`",
            "struct S {\n    n: i64\n    fn push(self, x: i64) { print(x) }\n    fn f(self) { self.push(1) }\n}\nfn main() {}\n"
                .to_string(),
            vec![],
        ),
        (
            "an enum's own plain `push` on `self`",
            "enum E {\n    A\n    fn push(self, x: i64) { print(x) }\n    fn f(self) { self.push(1) }\n}\nfn main() {}\n"
                .to_string(),
            vec![],
        ),
        (
            "`self` passed `mut` twice",
            "struct T {\n    n: i64\n    fn f(mut self) { two(mut self, mut self) }\n}\n\
             fn two(mut a: T, mut b: T) { a.n = 1; b.n = 1 }\nfn main() {}\n"
                .to_string(),
            vec![(MutPlaceTwice, (3, 40))],
        ),
        (
            "`bump(mut self)` in an `extend` of a struct",
            "struct P { x: i64 }\nextend P {\n    fn f(mut self) { bump(mut self) }\n}\n\
             fn bump(mut p: P) { p.x = 1 }\nfn main() {}\n"
                .to_string(),
            vec![],
        ),
        (
            "`bump(mut self)` in an `extend` of an enum",
            "enum E { A; B }\nextend E {\n    fn f(mut self) { bump(mut self) }\n}\n\
             fn bump(mut e: E) { e = E.B }\nfn main() {}\n"
                .to_string(),
            vec![],
        ),
        (
            "`self += 1` in a plain struct method",
            "struct T {\n    n: i64\n    fn f(self) { self += 1 }\n}\nfn main() {}\n".to_string(),
            vec![(AssignToSelf, (3, 18)), (Mismatch, (3, 18))],
        ),
        (
            "`self += 1` in a plain enum method",
            "enum E {\n    A\n    fn f(self) { self += 1 }\n}\nfn main() {}\n".to_string(),
            vec![(AssignToSelf, (3, 18)), (Mismatch, (3, 18))],
        ),
        (
            "`self += 1` in a `mut self` struct method",
            "struct T {\n    n: i64\n    fn f(mut self) { self += 1 }\n}\nfn main() {}\n"
                .to_string(),
            vec![(Mismatch, (3, 22))],
        ),
        (
            "`bump(mut self)`, then a `mut self` call",
            "struct T {\n    n: i64\n    fn f(mut self) { bump(mut self); self.g() }\n    fn g(mut self) { self.n += 1 }\n}\n\
             fn bump(mut t: T) { t.n += 1 }\nfn main() {}\n"
                .to_string(),
            vec![],
        ),
    ];
    for (row, src, wanted) in &rows {
        let both = format!("{src}{a}");
        let (_, ds) = checked_with("", &both);
        assert_eq!(errors(&both, &ds), *wanted, "row `{row}`");
    }
    // With `A` also declaring `fn len(mut self)`, a plain `len` on `self` takes the parked path,
    // and keeps the codes that it has without `A`.
    let a_len =
        "struct A { n: i64; fn bump(mut self) { self.n = 1 }; fn len(mut self) { self.n = 2 } }\n";
    let plain = [
        "struct S {\n    n: i64\n    fn len(self) -> i64 { self.n }\n    fn f(self) -> i64 { self.len() }\n}\nfn main() {}\n",
        "enum E {\n    A\n    fn len(self) -> i64 { 1 }\n    fn f(self) -> i64 { self.len() }\n}\nfn main() {}\n",
        "class C {\n    fn bump(self) {}\n    fn f(self) { self.bump() }\n}\nfn g() { self.bump() }\nfn main() {}\n",
    ];
    for src in plain {
        assert_eq!(codes_without(src, a_len), all_codes(src), "{src}");
    }
    // `bump(self)` in a `mut self` method: E0166 with the safe fix "add `mut`", and no W0002.
    let src = "struct S {\n    n: i64\n    fn f(mut self) { bump(self) }\n}\n\
               fn bump(mut s: S) { s.n = 1 }\nfn main() {}\n";
    let both = format!("{src}{a}");
    let (_, ds) = checked(&both);
    let found: Vec<_> = ds.iter().map(|d| (d.code(), at(&both, d.span()))).collect();
    assert_eq!(found, [(MutMarkerMissing, on(src, 3, "self) }"))]);
    assert_eq!(
        fixes(&both, &ds[0]),
        [(true, vec![(3, 27, "", "mut ".to_string())])],
        "{:?}",
        ds[0]
    );
}

/// GIVEN `class C` with its own plain `push`. WHEN a method runs `self.push(1)`. THEN the
/// program keeps its value path: W0013 at the field `n` only, as before.
#[test]
fn bare_self_push_in_a_class_should_keep_its_value_path() {
    judge_unused();
    let src = "class C {\n    n: i64\n    fn push(self, x: i64) { print(x) }\n    fn f(self) { self.push(1) }\n}\nfn main() {\n    C(n: 1).f()\n}\n";
    codes(src, &[(UnusedMember, (2, 5))]);
}

/// GIVEN `interface R { fn read(self) }`. WHEN `struct S: R` declares `fn read(mut self)`. THEN
/// `sisu check` reports E0152 at `read`.
#[test]
fn mut_self_method_should_not_meet_a_plain_requirement() {
    let src = "interface R {\n    fn read(self)\n}\nstruct S: R {\n    n: i64\n    fn read(mut self) { self.n += 1 }\n}\nfn main() {}";
    let (_, ds) = checked(src);
    assert_eq!(errors(src, &ds), [(ConformanceBody, (6, 8))]);
}
