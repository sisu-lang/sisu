//! `mut self` on a struct or an enum method: the declaration, the receiver of a call, and a bare
//! `self` as a place.

use super::parity::parity_found;
use super::*;
use Code::{
    Ambiguous, AssignToExpression, AssignToImmutable, AssignToSelf, BindingNotFound, CannotBeAny,
    CannotExtend, CannotInfer, ConformanceBody, ConformsTwice, Expected, FunctionNotFound,
    InterfaceAsType, MayBeNone, Mismatch, MutArgumentNotPlace, MutMarkerMissing, MutPlaceTwice,
    MutSelf, NeverReassigned, NoMember, SelfOutsideMethod, TypeArguments, UnchangedLoopCondition,
    UnexpectedReturnValue, UnknownType, UnusedMember, UnusedVariable,
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

/// GIVEN `struct P` with `fn show(mut self)`, `let p = P(x: 1)` and `let mut q = p`. WHEN `main`
/// writes `q.x = 2`, calls `q.show()` and prints `p.x`. THEN `sisu check` reports W0014 at `q`
/// only: the call writes the copy, so no W0016 says that the write is lost.
#[test]
fn mut_self_call_should_count_as_a_write_of_its_copy() {
    judge_unused();
    let src = "struct P { x: i64; fn show(mut self) { print(self.x); self.x = 0 } }\n\
               fn main() { let p = P(x: 1); let mut q = p; q.x = 2; q.show(); print(p.x) }";
    codes(src, &[(UnusedVariable, (2, 38))]);
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
    // The `let` row has the fix `let mut`, and the class field row the fix "mark the field
    // `mut`", as a `push` has.
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
    assert_eq!(
        fixes(&src, d),
        [(true, vec![(10, 11, "", "mut ".to_string())])],
        "{d:?}"
    );
}

/// The one diagnostic of `src` with the code `code`.
fn only(src: &str, code: Code) -> Diagnostic {
    let (_, ds) = checked(src);
    let mut found = ds.into_iter().filter(|d| d.code() == code);
    let d = found.next().expect("the program should have the code");
    assert!(found.next().is_none(), "{src}: a second {code:?}");
    d
}

/// GIVEN `Point` with `move_by`, `enum E { A; B; fn flip(mut self) }`, and an optional value.
/// WHEN the program runs a `mut self` call whose receiver's path holds a `?.`, or a `.` on an
/// optional. THEN `sisu check` reports one E0062, at the first `?.` or at the optional, and no
/// other error. A `.` on an optional gets the help and the fixes of the field write of the same
/// root, and no W0002.
#[test]
#[expect(clippy::too_many_lines, reason = "a table of rows")]
fn mut_self_call_through_an_optional_value_should_be_e0062() {
    judge_unused();
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
        assert_eq!(all_codes(src), [(MayBeNone, (17, 6))], "row `{row}`");
    }
    // A write through `?.` of a field of a `mut self`.
    let src = "struct In { n: i64 }\nfn bump(mut x: i64) { x = 1 }\n\
               struct P { n: i64; xs: [i64]; i: In; o: In? }\n\
               extend P {\n    fn m(mut self) { self.o?.n = 2 }\n}\nfn main() {}";
    assert_eq!(
        all_codes(src),
        [(MayBeNone, (5, 28))],
        "a write through `?.`"
    );
    // A receiver whose own check failed at a `?.` has its report: the call adds none. The
    // payload that a `.` on an optional checks the call on is no failure of its own. The call
    // writes the root, as the write of a field does, so the root has no W0002.
    let chained = [
        (
            "an index after the `?.`",
            "struct P { x: i64; fn m(mut self) { self.x = 1 } }\nstruct L { p: P; ps: [P] }\nfn main() { let mut o: L? = Some(L(p: P(x: 1), ps: [P(x: 1)])); o?.ps[0].m(); print(o != None) }",
            (MayBeNone, (3, 66)),
        ),
        (
            "an index after the `?.`, of an enum",
            "enum D { N; S; fn flip(mut self) { self = D.S } }\nstruct L { ds: [D] }\nfn main() { let mut o: L? = Some(L(ds: [D.N])); o?.ds[0].flip(); print(o != None) }",
            (MayBeNone, (3, 50)),
        ),
        (
            "`?.` on an optional of an optional",
            "struct P { x: i64; fn m(mut self) { self.x = 1 } }\nfn main() { let mut q: P?? = Some(Some(P(x: 1))); q?.m(); print(q != None) }",
            (MayBeNone, (2, 51)),
        ),
        (
            "a field after the `?.`",
            "struct P { x: i64; fn m(mut self) { self.x = 1 } }\nstruct L { p: P }\nfn main() { let mut o: L? = Some(L(p: P(x: 1))); o?.p.m(); print(o != None) }",
            (MayBeNone, (3, 51)),
        ),
        (
            "a field after the `?.`, of an enum",
            "enum D { N; S; fn flip(mut self) { self = D.S } }\nstruct L { d: D }\nfn main() { let mut o: L? = Some(L(d: D.N)); o?.d.flip(); print(o != None) }",
            (MayBeNone, (3, 47)),
        ),
        (
            "two `?.`",
            "struct P { x: i64; fn m(mut self) { self.x = 1 } }\nstruct L { p: P? }\nfn main() { let mut o: L? = Some(L(p: Some(P(x: 1)))); o?.p?.m(); print(o != None) }",
            (MayBeNone, (3, 57)),
        ),
        (
            "a struct's `push` after the `?.`",
            "struct S { n: i64; fn push(mut self, k: i64) { self.n += k } }\nstruct H { s: S }\nfn main() { let mut o: H? = None; o?.s.push(1); print(o != None) }",
            (MayBeNone, (3, 36)),
        ),
        (
            "`?.` on an optional of an optional enum",
            "enum E { A; B; fn flip(mut self) { self = E.B } }\nfn main() { let mut q: E?? = Some(Some(E.A)); q?.flip(); print(q != None) }",
            (MayBeNone, (2, 47)),
        ),
        (
            "an element of a `mut` parameter after the `?.`",
            "struct P { x: i64; fn m(mut self) { self.x = 1 } }\nstruct L { ps: [P] }\nfn f(mut o: L?) { o?.ps[0].m() }\nfn main() { let mut x: L? = None; f(mut x) }",
            (MayBeNone, (3, 20)),
        ),
        (
            "an element of an array of arrays before the `?.`",
            "struct P { x: i64; fn m(mut self) { self.x = 1 } }\nstruct L { ps: [P] }\nfn main() { let mut a: [[L?]] = [[None]]; a[0][0]?.ps[0].m(); print(a.len()) }",
            (MayBeNone, (3, 50)),
        ),
    ];
    for (row, src, wanted) in chained {
        assert_eq!(all_codes(src), [wanted], "row `{row}`");
    }
    // A field that is missing after the `?.` has its own error, and the call writes the root.
    let rows = [
        (
            "after the `?.`",
            "struct P { x: i64; fn bump(mut self) { self.x += 1 } }\nstruct L { p: P }\nfn main() { let mut o: L? = None; o?.zz.bump(); print(o != None) }",
            vec![(NoMember, (3, 38))],
        ),
        (
            "after an element",
            "struct P { x: i64; fn m(mut self) { self.x = 1 } }\nstruct L { ps: [P] }\nfn main() { let mut o: L? = None; o?.ps[0].zz.m(); print(o != None) }",
            vec![(MayBeNone, (3, 35)), (NoMember, (3, 44))],
        ),
    ];
    for (row, src, wanted) in rows {
        assert_eq!(all_codes(src), wanted, "a missing field {row}");
    }
    // A range index gives a copy, so the call writes no root and its W0002 stays.
    let rows = [
        (
            "after the `?.`",
            "struct P { x: i64; fn m(mut self) { self.x = 1 } }\nstruct L { ps: [P] }\nfn main() { let mut o: L? = None; o?.ps[0..1][0].m(); print(o != None) }",
            (3, 35),
        ),
        (
            "before the `?.`",
            "struct P { x: i64; fn m(mut self) { self.x = 1 } }\nstruct L { ps: [P] }\nfn main() { let mut ls: [L?] = [None]; ls[0..1][0]?.ps[0].m(); print(ls.len()) }",
            (3, 40),
        ),
    ];
    for (row, src, pos) in rows {
        assert_eq!(
            all_codes(src),
            [(NeverReassigned, (3, 21)), (MayBeNone, pos)],
            "a range index {row}"
        );
    }
    // A receiver that fails with no `?.` has its own error, and the call writes the root.
    let p = "struct P { x: i64; fn bump(mut self) { self.x += 1 } }\n";
    let rows = [
        (
            "an optional array",
            format!("{p}fn main() {{ let mut ps: [P]? = None; ps[0].bump(); print(ps != None) }}"),
            vec![(MayBeNone, (2, 38))],
        ),
        (
            "an optional array field",
            format!(
                "{p}struct L {{ ps: [P]? }}\nfn main() {{ let mut l = L(ps: None); l.ps[0].bump(); print(l.ps != None) }}"
            ),
            vec![(MayBeNone, (3, 38))],
        ),
        (
            "an unknown name in the index",
            format!(
                "{p}fn main() {{ let mut ps: [P] = [P(x: 1)]; ps[zz].bump(); print(ps.len()) }}"
            ),
            vec![(BindingNotFound, (2, 45))],
        ),
        (
            "an index of the wrong type",
            format!(
                "{p}fn main() {{ let mut ps: [P] = [P(x: 1)]; ps[true].bump(); print(ps.len()) }}"
            ),
            vec![(Mismatch, (2, 45))],
        ),
        (
            "an array's `push` on an optional array",
            "fn main() { let mut ps: [[i64]]? = None; ps[0].push(1); print(ps != None) }"
                .to_string(),
            vec![(MayBeNone, (1, 42))],
        ),
        (
            "a local whose initializer failed",
            format!("{p}fn main() {{ let mut p = nothere(); p.bump(); print(1) }}"),
            vec![(FunctionNotFound, (2, 25))],
        ),
        (
            "a local whose type and initializer failed",
            format!("{p}fn main() {{ let mut p: Nope = nothere(); p.bump(); print(1) }}"),
            vec![(UnknownType, (2, 24)), (FunctionNotFound, (2, 31))],
        ),
        (
            "a `let ... else` binding whose value failed",
            format!("{p}fn main() {{ let mut p = nothere() else {{ return }}; p.bump() }}"),
            vec![(FunctionNotFound, (2, 25))],
        ),
    ];
    for (row, src, wanted) in &rows {
        assert_eq!(all_codes(src), *wanted, "a failed receiver: {row}");
    }
    // The optional element `o?.pss[0]` reports E0062 at `o?.pss[0]` (as for a plain method)
    // and the `?.pss` chain reports `ChainCallOnValue` at `?.pss`. The call writes `o`, so `o`
    // has no W0002.
    let src = "struct P { x: i64; fn m(mut self) { self.x = 1 } }\nstruct L { pss: [P?] }\nfn main() { let mut o: L? = Some(L(pss: [None])); o?.pss[0].m(); print(o != None) }";
    assert_eq!(
        all_codes(src),
        [(MayBeNone, (3, 51)), (MayBeNone, (3, 52))],
        "an optional element"
    );
    // The peel help of `?.` on an optional of an optional stays, as on a field write.
    let src = chained[2].1;
    assert!(only(src, MayBeNone).help().is_some(), "{src}");
    // Each `.` row, and the field write whose help and fixes it has.
    let replace_o = "struct S { n: i64 }\nfn main() { let mut o: S? = Some(S(n: 1)); o.n = 2; print(o?.n ?? 0) }";
    let dotted = [
        (
            "a `let mut`",
            "struct S { n: i64; fn bump(mut self) { self.n += 1 } }\nfn main() { let mut o: S? = Some(S(n: 1)); o.bump(); print(o?.n ?? 0) }",
            (2, 44),
            replace_o,
        ),
        (
            "a method of an `extend`",
            "struct S { n: i64 }\nextend S { fn bump(mut self) { self.n += 1 } }\nfn main() { let mut o: S? = Some(S(n: 1)); o.bump(); print(o?.n ?? 0) }",
            (3, 44),
            replace_o,
        ),
        (
            "an enum",
            "enum D { N; S; fn flip(mut self) { self = D.S } }\nfn main() { let mut o: D? = Some(D.N); o.flip(); print(o != None) }",
            (2, 40),
            replace_o,
        ),
        (
            "a `let`",
            "struct S { n: i64; fn bump(mut self) { self.n += 1 } }\nfn main() { let o: S? = Some(S(n: 1)); o.bump(); print(o?.n ?? 0) }",
            (2, 40),
            "struct S { n: i64 }\nfn main() { let o: S? = Some(S(n: 1)); o.n = 2; print(o?.n ?? 0) }",
        ),
        (
            "a field of the optional",
            "struct P { x: i64; fn bump(mut self) { self.x += 1 } }\nstruct S { p: P }\nfn main() { let mut s: S? = None; s.p.bump(); print(s != None) }",
            (3, 35),
            "struct P { x: i64 }\nstruct S { p: P }\nfn main() { let mut s: S? = None; s.p.x = 2; print(s != None) }",
        ),
        (
            "an element of a field of the optional",
            "struct P { x: i64; fn bump(mut self) { self.x += 1 } }\nstruct L { ps: [P] }\nfn main() { let mut o: L? = None; o.ps[0].bump(); print(o != None) }",
            (3, 35),
            "struct P { x: i64 }\nstruct L { ps: [P] }\nfn main() { let mut o: L? = None; o.ps[0].x = 1; print(o != None) }",
        ),
        (
            "an optional of an optional",
            "struct S { n: i64; fn bump(mut self) { self.n += 1 } }\nfn main() { let mut o: S?? = None; o.bump(); print(o != None) }",
            (2, 36),
            "struct S { n: i64 }\nfn main() { let mut o: S?? = None; o.n = 1; print(o != None) }",
        ),
        (
            "an element",
            "struct S { n: i64; fn bump(mut self) { self.n += 1 } }\nfn main() { let mut xs: [S?] = [None]; xs[0].bump(); print(xs.len()) }",
            (2, 40),
            "struct S { n: i64 }\nfn main() { let mut xs: [S?] = [None]; xs[0].n = 1; print(xs.len()) }",
        ),
        (
            "a `mut` parameter",
            "struct S { n: i64; fn bump(mut self) { self.n += 1 } }\nfn f(mut o: S?) { o.bump(); print(o?.n ?? 0) }\nfn main() { let mut x: S? = None; f(mut x) }",
            (2, 19),
            "struct S { n: i64 }\nfn f(mut o: S?) { o.n = 1; print(o?.n ?? 0) }\nfn main() { let mut x: S? = None; f(mut x) }",
        ),
        (
            "a field of `mut self`",
            "struct S { n: i64; fn bump(mut self) { self.n += 1 } }\nstruct H { o: S?; fn g(mut self) { self.o.bump() } }\nfn main() { let mut h = H(o: None); h.g(); print(h.o?.n ?? 0) }",
            (2, 36),
            "struct S { n: i64 }\nstruct H { o: S?; fn g(mut self) { self.o.n = 1 } }\nfn main() { let mut h = H(o: None); h.g(); print(h.o?.n ?? 0) }",
        ),
        (
            "a struct's `push`",
            "struct S { n: i64; fn push(mut self, k: i64) { self.n += k } }\nfn main() { let mut o: S? = None; o.push(1); print(o?.n ?? 0) }",
            (2, 35),
            replace_o,
        ),
        (
            "a struct's `push` on a field of the optional",
            "struct S { n: i64; fn push(mut self, k: i64) { self.n += k } }\nstruct H { s: S }\nfn main() { let mut o: H? = None; o.s.push(1); print(o != None) }",
            (3, 35),
            replace_o,
        ),
        (
            "an element of an element of a field",
            "struct P { x: i64; fn len(mut self) -> i64 { self.x = 1; 1 } }\nstruct L { pss: [[P]] }\nfn main() { let mut o: L? = None; print(o.pss[0][0].len()) }",
            (3, 41),
            "struct P { x: i64 }\nstruct L { pss: [[P]] }\nfn main() { let mut o: L? = None; o.pss[0][0].x = 2; print(o != None) }",
        ),
        (
            "a field of a field",
            "struct P { x: i64; fn len(mut self) -> i64 { self.x = 1; 1 } }\nstruct Q { p: P; ps: [P] }\nstruct L { q: Q }\nfn main() { let mut o: L? = None; print(o.q.p.len()) }",
            (4, 41),
            replace_o,
        ),
        (
            "an element of a field of a field",
            "struct P { x: i64; fn len(mut self) -> i64 { self.x = 1; 1 } }\nstruct Q { p: P; ps: [P] }\nstruct L { q: Q }\nfn main() { let mut o: L? = None; print(o.q.ps[0].len()) }",
            (4, 41),
            replace_o,
        ),
        (
            "a `push` onto an array field of a field",
            "struct P { x: i64; fn len(mut self) -> i64 { self.x = 1; 1 } }\nstruct Q { p: P; ps: [P] }\nstruct L { q: Q }\nfn main() { let mut o: L? = None; o.q.ps.push(P(x: 1)); print(o != None) }",
            (4, 35),
            replace_o,
        ),
    ];
    for (row, src, pos, twin) in dotted {
        assert_eq!(all_codes(src), [(MayBeNone, pos)], "row `{row}`");
        // The fixture `E0062-mut-self-call` pins the text of the help.
        let shape = |d: &Diagnostic| (d.help().is_some(), d.fixes().len());
        let (d, write) = (only(src, MayBeNone), only(twin, MayBeNone));
        assert_eq!(
            shape(&d),
            shape(&write),
            "row `{row}`: the help and the fixes"
        );
    }
    // A plain method keeps the read's help and its `?.` fix.
    let src = "struct S { n: i64; fn show(self) { print(self.n) }; fn bump(mut self) { self.n += 1 } }\nfn main() { let mut o: S? = Some(S(n: 1)); o.show(); print(o?.n ?? 0) }";
    assert_eq!(
        all_codes(src),
        [(NeverReassigned, (2, 21)), (MayBeNone, (2, 44))],
        "a plain method"
    );
    let d = only(src, MayBeNone);
    assert!(d.help().is_some(), "{d:?}");
    assert_eq!(d.fixes().len(), 1, "{d:?}");
}

/// GIVEN `struct A { n: i64; fn len(mut self) }`, after a program whose plain methods share the
/// name `len`. WHEN the program is checked. THEN each row has the codes of the same program
/// without `A`: the receiver of a plain method keeps its rules.
#[test]
#[expect(clippy::too_many_lines, reason = "a table of rows")]
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
            "fn main() {\n    let o: [i64]? = Some([1])\n    print(o?.len() ?? 0)\n}\n",
            vec![],
        ),
        (
            "a struct field that nothing else reads",
            "struct P { x: i64; fn len(self) -> i64 { self.x } }\nstruct S { p: P }\nfn main() {\n    let s = S(p: P(x: 1))\n    print(s.p.len())\n}\n",
            vec![],
        ),
        (
            "a field read as a comparison",
            "struct W { len: i64 }\nfn main() {\n    let w = W(len: 1)\n    let t = 2\n    print(w.len<i64>(t))\n}\n",
            vec![(TypeArguments, (5, 11))],
        ),
        (
            "a built-in's method read as a comparison",
            "fn main() {\n    let a1 = 1\n    let b1 = 2\n    let xs = [1]\n    print(xs.len<a1, b1>(3))\n}\n",
            vec![(TypeArguments, (5, 11))],
        ),
        (
            "an array field of an optional",
            "struct S { xs: [i64] }\nfn main() {\n    let mut s: S? = None\n    print(s.xs.len())\n}\n",
            vec![(NeverReassigned, (3, 13)), (MayBeNone, (4, 11))],
        ),
    ];
    for (row, src, wanted) in &rows {
        assert_eq!(all_codes(src), *wanted, "row `{row}` without `A`");
        assert_eq!(codes_without(src, a), *wanted, "row `{row}` with `A`");
    }
    // `P` has `fn len(mut self)`. An array's own `len` on a path of an optional is a read. It has
    // the codes, the help and the fixes of the twin where `P` has no `len`.
    let no_len = |src: &str| src.replace("; fn len(mut self) -> i64 { self.x = 1; 1 }", "");
    let p = "struct P { x: i64; fn len(mut self) -> i64 { self.x = 1; 1 } }\n";
    let rows = [
        (
            "an element of an array of arrays",
            format!(
                "{p}struct L {{ pss: [[P]] }}\nfn main() {{ let mut o: L? = None; print(o.pss[0].len()) }}"
            ),
            vec![(NeverReassigned, (3, 21)), (MayBeNone, (3, 41))],
        ),
        (
            "an element of an array of arrays in a `let`",
            format!(
                "{p}struct L {{ pss: [[P]] }}\nfn main() {{ let o: L? = None; print(o.pss[0].len()) }}"
            ),
            vec![(MayBeNone, (3, 37))],
        ),
        (
            "an element of an element of an array of three",
            format!(
                "{p}struct L {{ psss: [[[P]]] }}\nfn main() {{ let mut o: L? = None; print(o.psss[0][0].len()) }}"
            ),
            vec![(NeverReassigned, (3, 21)), (MayBeNone, (3, 41))],
        ),
        (
            "an element of an array of arrays in a parameter",
            format!(
                "{p}struct L {{ pss: [[P]] }}\nfn f(o: L?) -> i64 {{ o.pss[0].len() }}\nfn main() {{ print(f(None)) }}"
            ),
            vec![(MayBeNone, (3, 22))],
        ),
        (
            "an array of arrays",
            format!(
                "{p}struct L {{ pss: [[P]] }}\nfn main() {{ let mut o: L? = None; print(o.pss.len()) }}"
            ),
            vec![(NeverReassigned, (3, 21)), (MayBeNone, (3, 41))],
        ),
        (
            "a range of an array",
            format!(
                "{p}struct L {{ ps: [P] }}\nfn main() {{ let mut o: L? = None; print(o.ps[0..1].len()) }}"
            ),
            vec![(NeverReassigned, (3, 21)), (MayBeNone, (3, 41))],
        ),
        (
            "a range of an element of an array of arrays",
            format!(
                "{p}struct L {{ pss: [[P]] }}\nfn main() {{ let mut o: L? = None; print(o.pss[0][0..1].len()) }}"
            ),
            vec![(NeverReassigned, (3, 21)), (MayBeNone, (3, 41))],
        ),
        (
            "an array field of a field",
            format!(
                "{p}struct Q {{ ps: [P] }}\nstruct L {{ q: Q }}\nfn main() {{ let mut o: L? = None; print(o.q.ps.len()) }}"
            ),
            vec![(NeverReassigned, (4, 21)), (MayBeNone, (4, 41))],
        ),
        (
            "an array field of an element of a field",
            format!(
                "{p}struct Q {{ ps: [P] }}\nstruct L {{ qs: [Q] }}\nfn main() {{ let mut o: L? = None; print(o.qs[0].ps.len()) }}"
            ),
            vec![(NeverReassigned, (4, 21)), (MayBeNone, (4, 41))],
        ),
    ];
    let shape = |src: &str| {
        let d = only(src, MayBeNone);
        (d.help().is_some(), d.fixes().len())
    };
    for (row, src, wanted) in &rows {
        let twin = no_len(src);
        assert_eq!(all_codes(&twin), *wanted, "row `{row}` without `len`");
        assert_eq!(all_codes(src), *wanted, "row `{row}`");
        assert_eq!(
            shape(src),
            shape(&twin),
            "row `{row}`: the help and the fixes"
        );
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
    // The other named change: a literal argument of such a method takes its default type.
    let src = "struct Bx { v: i64; fn m<U>(self, u: U) -> U { u } }\nfn id<T>(t: T) -> T { t }\n\
               fn main() {\n    let b = Bx(v: 1)\n    let u: u8 = id(b.m(1))\n    print(u)\n}\n";
    let (_, ds) = checked(src);
    assert_eq!(errors(src, &ds), [], "without `A`");
    let a_m = "struct A { n: i64; fn m(mut self) { self.n = 1 } }\n";
    let both = format!("{src}{a_m}");
    let (_, ds) = checked(&both);
    assert_eq!(
        errors(&both, &ds),
        [(Mismatch, on(src, 5, "b.m(1)"))],
        "with `A`"
    );
    // A generated `eq` and `less`, and an array's `get` with a range, read the receiver that a
    // `mut self` method of the same name parks.
    let ops = "struct A { n: i64; fn eq(mut self, o: i64) -> bool { self.n += o; true }; fn less(mut self, o: i64) -> bool { self.n += o; false }; fn get(mut self, i: i64) -> i64 { self.n += i; self.n } }\n";
    let rows = [
        (
            "an `i64` `let`",
            "fn main() {\n    let x = 1\n    print(x.eq(1))\n    print(x.less(2))\n}\n",
        ),
        (
            "an array",
            "fn main() {\n    let xs = [1, 2]\n    print(xs.eq([1, 2]))\n}\n",
        ),
        (
            "a string parameter",
            "fn f(s: string) -> bool { s.eq(\"b\") }\nfn main() {\n    print(f(\"a\"))\n}\n",
        ),
        (
            "a struct field of a `let mut`",
            "struct Q { y: i64 }\nstruct W { q: Q }\nfn main() {\n    let mut w = W(q: Q(y: 1))\n    w = W(q: Q(y: 2))\n    print(w.q.eq(Q(y: 2)))\n}\n",
        ),
        (
            "an enum",
            "enum E { X; Y }\nfn main() {\n    let e = E.X\n    print(e.eq(E.Y))\n}\n",
        ),
        (
            "a `for` binding",
            "fn main() {\n    for x in [1, 2] {\n        print(x.eq(1))\n    }\n}\n",
        ),
        (
            "an element",
            "fn main() {\n    let xs = [[1], [2]]\n    print(xs[0].eq([1]))\n}\n",
        ),
        (
            "an optional",
            "fn main() {\n    let x: i64? = Some(1)\n    print(x.eq(Some(1)))\n}\n",
        ),
        (
            "a field of `self`",
            "struct B { k: i64; fn f(self) -> bool { self.k.eq(1) } }\nfn main() {\n    print(B(k: 1).f())\n}\n",
        ),
        (
            "a type parameter",
            "fn g<T: Eq>(t: T, u: T) -> bool { t.eq(u) }\nfn main() {\n    print(g(1, 2))\n}\n",
        ),
        (
            "type arguments",
            "fn main() {\n    let x = 1\n    print(x.eq<i64>(1))\n}\n",
        ),
        (
            "a struct's own `mut self` `eq`",
            "struct P { x: i64; fn eq(mut self, o: P) -> bool { self.x = o.x; true } }\nfn main() {\n    let mut p = P(x: 1)\n    print(p.eq(P(x: 2)))\n    print(p.x)\n}\n",
        ),
        (
            "an array's `get` with a range",
            "fn main() {\n    let xs = [1, 2, 3]\n    print(xs.get(0..1))\n}\n",
        ),
    ];
    for (row, src) in rows {
        assert_eq!(codes_without(src, ops), all_codes(src), "row `{row}`");
    }
}

/// GIVEN a method body with `mut self` in a struct, an enum, an `extend` of each, or a class.
/// WHEN the program is checked. THEN `mut self` has no error where it has meaning, and each
/// other form keeps its code.
#[test]
#[expect(clippy::too_many_lines, reason = "a table of rows")]
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
            "an `extend` of an alias of a struct",
            "struct P { x: i64 }\ntype A = P\nextend A { fn m(mut self) { self.x += 1 } }\n\
             fn main() { let mut p = P(x: 1); p.m(); print(p.x) }"
                .to_string(),
            vec![],
        ),
        (
            "an `extend` of an alias of a class",
            "class C { mut n: i64 }\ntype CA = C\nextend CA { fn m(mut self) {} }\n\
             fn main() { let c = C(n: 1); print(c.n) }"
                .to_string(),
            vec![(MutSelf, (3, 18))],
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
#[expect(clippy::too_many_lines, reason = "a table of rows")]
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
        (
            "`push` on a call in a struct method",
            "struct T {\n    n: i64\n    fn m(self) {\n        make().push(1)\n    }\n}\n\
             fn make() -> [i64] { [1] }\nfn main() {\n    T(n: 1).m()\n}\n"
                .to_string(),
            vec![(AssignToExpression, (4, 9))],
        ),
        (
            "`push` on a call in an enum method",
            "enum E {\n    A\n    fn m(self) {\n        make().push(1)\n    }\n}\n\
             fn make() -> [i64] { [1] }\nfn main() {\n    E.A.m()\n}\n"
                .to_string(),
            vec![(AssignToExpression, (4, 9))],
        ),
        (
            "a `mut self` call on a call in a struct method",
            "struct T {\n    n: i64\n    fn m(self) {\n        mk().bump()\n    }\n}\n\
             fn mk() -> A { A(n: 0) }\nfn main() {\n    T(n: 1).m()\n}\n"
                .to_string(),
            vec![(AssignToExpression, (4, 9))],
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

/// GIVEN `interface Bump { fn bump(mut self) }`, `struct H<T> { v: T }` and `struct N: Bump`.
/// WHEN `fn g<T: Bump>(mut h: H<T>?)` runs `h.v.bump()`. THEN `sisu check` reports one E0062 at
/// `h`, with no fix, and no W0002: the call may write the generic part.
#[test]
fn mut_self_call_on_a_generic_part_behind_an_optional_should_be_e0062_alone() {
    judge_unused();
    let src = "interface Bump {\n    fn bump(mut self)\n}\nstruct H<T> {\n    v: T\n}\n\
               struct N {\n    n: i64\n}\nextend N: Bump {\n    fn bump(mut self) { self.n = self.n + 1 }\n}\n\
               fn g<T: Bump>(mut h: H<T>?) {\n    h.v.bump()\n}\nfn main() {}\n";
    let (_, ds) = checked(src);
    let found: Vec<_> = ds.iter().map(|d| (d.code(), at(src, d.span()))).collect();
    assert_eq!(found, [(MayBeNone, (14, 5))]);
    assert_eq!(fixes(src, &ds[0]), [], "{:?}", ds[0]);
}

/// GIVEN a struct, an `extend` block or an enum whose `mut self` method `m` has a syntax error in
/// its body. WHEN `main` calls `m` on a local. THEN `sisu check` reports the syntax error alone:
/// the call may write the local, so it gets no W0002 and no W0014, and a `let` gets no E0001.
#[test]
fn call_of_a_broken_method_should_report_only_the_method_error() {
    judge_unused();
    let s = "struct T {\n    n: i64\n    fn m(mut self) {\n        self.n = 2\n        f() = 3\n    }\n}\n\
             fn f() -> i64 { 1 }\n";
    let e = "struct T {\n    n: i64\n}\nextend T {\n    fn m(mut self) {\n        self.n = 2\n        f() = 3\n    }\n}\n\
             fn f() -> i64 { 1 }\n";
    let plain = "struct T {\n    n: i64\n    fn m(self) {\n        print(self.n)\n        f() = 3\n    }\n}\n\
                 fn f() -> i64 { 1 }\n";
    let call = "fn main() {\n    let mut t = T(n: 1)\n    t.m()\n    print(t.n)\n}\n";
    let rows = [
        ("a struct method", format!("{s}{call}"), (AssignToExpression, (5, 9))),
        (
            "a missing value",
            format!("{s}{call}").replace("f() = 3", "self.n = "),
            (Expected, (6, 5)),
        ),
        ("a method of an `extend` block", format!("{e}{call}"), (AssignToExpression, (7, 9))),
        (
            "an enum method",
            "enum E {\n    A\n    B\n    fn m(mut self) {\n        self = E.B\n        let = 1\n    }\n}\n\
             fn main() {\n    let mut e = E.A\n    e.m()\n    print(1)\n}\n"
                .to_string(),
            (Expected, (6, 13)),
        ),
        ("a `let` caller", format!("{s}{call}").replace("let mut t", "let t"), (AssignToExpression, (5, 9))),
        ("a caller that never reads", format!("{s}{call}").replace("    print(t.n)\n", ""), (AssignToExpression, (5, 9))),
        ("a plain method", format!("{plain}{call}"), (AssignToExpression, (5, 9))),
    ];
    for (row, src, wanted) in rows {
        let ds = crate::front_end::front_end(&src).1;
        let found: Vec<_> = ds.iter().map(|d| (d.code(), at(&src, d.span()))).collect();
        assert_eq!(found, [wanted], "row `{row}`");
    }
}

/// GIVEN a `mut self` method that never changes `self`, and a call that needs the `mut` for a
/// W0014 or an E0050. WHEN each safe fix is applied alone. THEN it keeps the contract of
/// `safe_fix_breaches`: the W0002 fix of `self` is unsafe there.
#[test]
fn unused_mut_self_safe_fixes_should_keep_their_contract() {
    let programs = [
        "struct Pile {\n    n: i64\n    fn show(mut self) { print(self.n) }\n}\nfn main() {\n    let mut pile = Pile(n: 0)\n    pile.show()\n}\n",
        "struct Pile {\n    n: i64\n    fn peek(mut self) { print(self.n) }\n}\nstruct W {\n    p: Pile\n}\nfn main() {\n    let mut w = W(p: Pile(n: 1))\n    w.p.peek()\n    let mut c = w\n    c.p.peek()\n}\n",
        "struct B {\n    n: i64\n    fn none<U>(mut self) -> U? { None }\n}\nfn id<U>(u: U) -> U { u }\nfn main() {\n    let mut b = B(n: 1)\n    b.n = 2\n    let x: i64? = id(b.none())\n    print(x ?? 0)\n}\n",
        "struct A {\n    n: i64\n    fn none(mut self) {}\n}\nstruct B {\n    n: i64\n    fn none<U>(self) -> U? { None }\n}\nfn id<U>(u: U) -> U { u }\nfn main() {\n    let mut a = A(n: 1)\n    a.none()\n    print(a.n)\n    let b = B(n: 1)\n    let x: i64? = id(b.none())\n    print(x ?? 0)\n}\n",
    ];
    for src in programs {
        let breaches = crate::on_compiler_stack(|| crate::front_end::safe_fix_breaches(src));
        assert_eq!(breaches, Vec::<String>::new(), "{src}");
    }
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

/// The one error of `src`, which has `code` at `at`. With `take`, it has one unsafe fix that
/// inserts `mut ` there, and no help. Without it, it has no fix and a help.
fn plain_self_error(row: &str, src: &str, (code, at): Wanted, take: Option<(usize, usize)>) {
    let (_, ds) = checked(src);
    assert_eq!(errors(src, &ds), [(code, at)], "row `{row}`");
    let d = ds
        .iter()
        .find(|d| d.code() == code)
        .expect("the row should have its code");
    if let Some((line, col)) = take {
        let want = vec![(false, vec![(line, col, "", "mut ".to_string())])];
        assert_eq!(fixes(src, d), want, "row `{row}`: {d:?}");
        assert!(d.help().is_none(), "row `{row}`: {d:?}");
    } else {
        assert!(d.fixes().is_empty(), "row `{row}`: {d:?}");
        assert!(d.help().is_some(), "row `{row}`: {d:?}");
    }
}

/// One row of `plain_self_error`: its name, its program, its error, and where its fix inserts.
type PlainSelfRow = (&'static str, &'static str, Wanted, Option<(usize, usize)>);

/// GIVEN a method with plain `self` that has `self = e`, in a struct, an enum, an `extend`, a
/// class, or behind a plain `self` requirement. WHEN `sisu check` runs on it. THEN it reports
/// E0073 at `self`. A struct or an enum has the unsafe fix "take `mut self`". A method that a
/// requirement binds has the help to copy `self`. A class has its own text.
#[test]
fn assignment_to_plain_self_should_give_the_help_of_its_kind() {
    let rows: [PlainSelfRow; 5] = [
        (
            "a struct",
            "struct T {\n    n: i64\n    fn f(self) { self = T(n: 1) }\n}\nfn main() {}",
            (AssignToSelf, (3, 18)),
            Some((3, 10)),
        ),
        (
            "an enum",
            "enum E {\n    A\n    B\n    fn f(self) { self = E.B }\n}\nfn main() {}",
            (AssignToSelf, (4, 18)),
            Some((4, 10)),
        ),
        (
            "a plain `self` requirement",
            "interface R {\n    fn f(self)\n}\nstruct T: R {\n    n: i64\n    fn f(self) { self = T(n: 1) }\n}\nfn main() {}",
            (AssignToSelf, (6, 18)),
            None,
        ),
        (
            "an `extend` of a struct",
            "struct P { x: i64 }\nextend P {\n    fn f(self) { self = P(x: 1) }\n}\nfn main() {}",
            (AssignToSelf, (3, 18)),
            Some((3, 10)),
        ),
        (
            "an `extend` of a class",
            "class C {\n    mut n: i64\n}\nextend C {\n    fn f(self) { self = C(n: 1) }\n}\nfn main() {}",
            (AssignToSelf, (5, 18)),
            None,
        ),
    ];
    for (row, src, wanted, take) in rows {
        plain_self_error(row, src, wanted, take);
    }
    // A class body is a parse error of E0073 alone, with its own help and no fix.
    let src = "class C {\n    mut n: i64\n    fn f(self) { self = C(n: 1) }\n}\nfn main() {}";
    let parsed = crate::parser::parse(
        crate::lexer::lex(src).expect("the source should lex"),
        false,
    );
    let found: Vec<_> = (parsed.diagnostics.iter())
        .map(|d| {
            (
                d.code(),
                at(src, d.span()),
                d.fixes().len(),
                d.help().is_some(),
            )
        })
        .collect();
    assert_eq!(found, [(AssignToSelf, (3, 18), 0, true)], "a class body");
    // A built-in holder has E0155 alone, whatever its `mut self` method writes.
    let src = "extend i64 {\n    fn inc(mut self) { self += 1 }\n}\nfn main() {}";
    let (_, ds) = checked(src);
    assert_eq!(errors(src, &ds), [(CannotExtend, (1, 8))], "a built-in");
}

/// The program of `change_through_plain_self_should_offer_mut_self`: `body` in the plain method
/// `f` of `T`, at line 12, which meets a plain `self` requirement when `bound` holds.
fn change_program(body: &str, bound: bool) -> String {
    let conforms = if bound { ": R" } else { "" };
    format!(
        "struct Q {{\n    xs: [i64]\n}}\nfn bp(mut q: Q) {{}}\nfn bu(mut t: T) {{}}\n\
         interface R {{\n    fn f(self)\n}}\nstruct T{conforms} {{\n    n: i64\n    p: Q\n    fn f(self) {{ {body} }}\n}}\nfn main() {{}}\n"
    )
}

/// GIVEN a plain `self` method of a struct. It changes `self` or a part of it, passes it `mut`,
/// or calls a `mut self` method on it. WHEN `sisu check` runs on it. THEN it reports the row's
/// code at the row's place, with the unsafe fix "take `mut self`". A method that a plain `self`
/// requirement binds gets the help to copy `self` in place of the fix.
#[test]
fn change_through_plain_self_should_offer_mut_self() {
    // The program has `fn f(self)` at line 12, and the body starts at column 18 of it.
    let rows = [
        ("a field write", "self.n = 1", AssignToImmutable, 0),
        ("a `push`", "self.p.xs.push(1)", AssignToImmutable, 0),
        (
            "a `mut` argument on a part",
            "bp(mut self.p)",
            MutArgumentNotPlace,
            7,
        ),
        (
            "a `mut` argument on `self`",
            "bu(mut self)",
            MutArgumentNotPlace,
            7,
        ),
    ];
    for (row, body, code, offset) in rows {
        for bound in [false, true] {
            let src = change_program(body, bound);
            let take = (!bound).then_some((12, 10));
            let name = format!("{row}, bound: {bound}");
            plain_self_error(&name, &src, (code, (12, 18 + offset)), take);
        }
    }
    // A `mut self` call on `self`, and on a field of it.
    let program = |holder: &str, body: &str, bound: bool| {
        let conforms = if bound { ": R" } else { "" };
        format!(
            "{POINT}struct L {{ p: Point; ps: [Point] }}\ninterface R {{\n    fn f(self)\n}}\n\
             extend {holder}{conforms} {{\n    fn f(self) {{ {body} }}\n}}\nfn main() {{}}\n"
        )
    };
    for bound in [false, true] {
        let take = (!bound).then_some((14, 10));
        let src = program("Point", "self.move_by(1, 1)", bound);
        plain_self_error(
            "a `mut self` call",
            &src,
            (AssignToImmutable, (14, 18)),
            take,
        );
        let src = program("L", "self.p.move_by(1, 1)", bound);
        plain_self_error(
            "a call on a field",
            &src,
            (AssignToImmutable, (14, 18)),
            take,
        );
    }
    // A class keeps its text: `self` is a place that no `mut self` can offer.
    let src = "class C {\n    mut n: i64\n    fn f(self) { bump(mut self) }\n}\nfn bump(mut c: C) {}\nfn main() {}";
    plain_self_error("a class", src, (MutArgumentNotPlace, (3, 27)), None);
}

/// GIVEN `enum E { A; B; fn f(self) { be(mut self) } }` and `fn be(mut e: E) {}`. WHEN `sisu
/// check` runs on it. THEN it reports E0169 at `self`, with the unsafe fix "take `mut self`".
#[test]
fn mut_argument_on_enum_self_should_offer_mut_self() {
    let src = "enum E {\n    A\n    B\n    fn f(self) { be(mut self) }\n}\nfn be(mut e: E) {}\nfn main() {}";
    plain_self_error(
        "an enum",
        src,
        (MutArgumentNotPlace, (4, 25)),
        Some((4, 10)),
    );
}

/// GIVEN a method that takes `mut self` and a `mut` argument. WHEN the argument is the receiver's
/// place, is inside it, or holds it. THEN `sisu check` reports E0171 at the argument, with the
/// receiver's place as its other span, and no fix.
#[test]
fn receiver_should_be_the_first_mut_slot() {
    let rows = [
        (
            "the same place",
            "struct P {\n    n: i64\n    fn change(mut self, mut q: P) { self.n = 1; q.n = 1 }\n}\nfn main() {\n    let mut p = P(n: 0)\n    p.change(mut p)\n}\n",
            (7, 18),
            (7, 5),
        ),
        (
            "the argument is inside the receiver",
            "struct Q {\n    n: i64\n}\nstruct L {\n    c: Q\n    fn change(mut self, mut q: Q) { self.c.n = 1; q.n = 1 }\n}\nfn main() {\n    let mut l = L(c: Q(n: 0))\n    l.change(mut l.c)\n}\n",
            (10, 18),
            (10, 5),
        ),
        (
            "the receiver is inside the argument",
            "struct L {\n    c: Q\n}\nstruct Q {\n    n: i64\n    fn change(mut self, mut l: L) { self.n = 1; l.c.n = 1 }\n}\nfn main() {\n    let mut l = L(c: Q(n: 0))\n    l.c.change(mut l)\n}\n",
            (10, 20),
            (10, 5),
        ),
    ];
    for (row, src, arg, receiver) in rows {
        let (_, ds) = checked(src);
        assert_eq!(errors(src, &ds), [(MutPlaceTwice, arg)], "row `{row}`");
        let d = &ds[0];
        let spans: Vec<_> = d.secondary().iter().map(|(s, _)| at(src, *s)).collect();
        assert_eq!(spans, [receiver], "row `{row}`: the receiver");
        assert!(d.fixes().is_empty(), "row `{row}`: {d:?}");
    }
}

/// `interface K { fn add(mut self) }`, on one line, before the row's items.
const K_ADD: &str = "interface K { fn add(mut self) }\n";

/// GIVEN `interface K { fn add(mut self) }` and a type that conforms to it. WHEN `sisu check` runs
/// on it. THEN it reports the row's errors: none for a struct, an enum or a class that has `add`,
/// and E0152 at `K` for a class without it. The generic struct's `mut self` gets no W0002.
#[test]
fn interface_should_accept_a_mut_self_requirement() {
    let program = |items: &str| format!("{K_ADD}{items}\nfn main() {{}}");
    let rows: Vec<(&str, String, Vec<Wanted>)> = vec![
        (
            "a struct's `mut self`",
            program("struct S: K { n: i64; fn add(mut self) { self.n += 1 } }"),
            vec![],
        ),
        (
            "plain `self` meets it",
            program("struct S: K { fn add(self) {} }"),
            vec![],
        ),
        (
            "a conformance in `extend`",
            program("struct S { n: i64 }\nextend S: K { fn add(mut self) { self.n += 1 } }"),
            vec![],
        ),
        (
            "a generic struct",
            program("struct G<T> { v: T }\nextend<T> G<T>: K { fn add(mut self) {} }"),
            vec![],
        ),
        (
            "a class's plain `self`",
            program("class C: K { mut n: i64; fn add(self) { self.n += 1 } }"),
            vec![],
        ),
        (
            "a class without `add`",
            program("class C: K {}"),
            vec![(ConformanceBody, (2, 10))],
        ),
        (
            "an enum's `mut self`",
            program("enum E: K { A; fn add(mut self) { self = E.A } }"),
            vec![],
        ),
    ];
    error_rows(&rows);
    // A requirement keeps the `mut` of its method, so W0002 leaves it alone.
    let (_, ds) = checked_with(PRELUDE_SOURCE, &rows[3].1);
    assert!(ds.iter().all(|d| d.code() != NeverReassigned), "{ds:?}");
}

/// GIVEN an interface whose requirement takes plain `self`, and a type whose method takes
/// `mut self`. WHEN `sisu check` runs on it. THEN it reports E0152 at the method's name. For a
/// written interface, the diagnostic has a note.
#[test]
fn method_should_match_the_self_mark_of_its_requirement() {
    let rows: Vec<(&str, String, Vec<Wanted>)> = vec![
        (
            "`mut self` asks for more",
            "interface R { fn read(self) }\nstruct S: R { fn read(mut self) {} }\nfn main() {}"
                .to_string(),
            vec![(ConformanceBody, (2, 18))],
        ),
        (
            "a language interface",
            "struct S: Display { fn to_string(mut self) -> string { \"s\" } }\nfn main() {}"
                .to_string(),
            vec![(ConformanceBody, (1, 24))],
        ),
    ];
    error_rows(&rows);
    let (_, ds) = checked_with(PRELUDE_SOURCE, &rows[0].1);
    let d = only_code(&ds, ConformanceBody);
    assert_eq!(d.notes().len(), 1, "{d:?}");
}

/// The one diagnostic of `ds` with `code`.
fn only_code(ds: &[Diagnostic], code: Code) -> &Diagnostic {
    let mut found = ds.iter().filter(|d| d.code() == code);
    let d = found.next().expect("the program should have the code");
    assert!(found.next().is_none(), "{ds:?}");
    d
}

/// GIVEN `interface K { fn add(mut self) }` and `struct S: K` whose `add` takes plain `self` and
/// writes `self.n`. WHEN `sisu check` runs on it. THEN it reports E0001 at `self` with the unsafe
/// fix "take `mut self`": the requirement allows `mut self`, so no copy is needed.
#[test]
fn method_for_a_mut_self_requirement_should_offer_mut_self() {
    let src = "interface K {\n    fn add(mut self)\n}\nstruct S: K {\n    n: i64\n    fn add(self) { self.n = 1 }\n}\nfn main() {}";
    plain_self_error(
        "a `mut self` requirement",
        src,
        (AssignToImmutable, (6, 20)),
        Some((6, 12)),
    );
}

/// GIVEN a class method with `mut self`, in the class body or in an `extend`. WHEN `sisu check`
/// runs on it. THEN it reports E0018 at `mut`.
#[test]
fn class_should_still_reject_mut_self() {
    let rows: Vec<(&str, String, Vec<Wanted>)> = vec![
        (
            "the body",
            "class C { fn f(mut self) {} }\nfn main() {}".to_string(),
            vec![(MutSelf, (1, 16))],
        ),
        (
            "`extend`",
            "class C {}\nextend C { fn f(mut self) {} }\nfn main() {}".to_string(),
            vec![(MutSelf, (2, 17))],
        ),
    ];
    error_rows(&rows);
}

/// GIVEN `interface K { fn add(mut self) }`. WHEN the program has `fn bump<T: K>(t: T) { t.add()
/// }`. THEN `sisu check` reports E0001 at `t` of the call.
#[test]
fn plain_parameter_of_a_bound_should_not_take_a_mut_self_call() {
    let src = format!("{K_ADD}fn bump<T: K>(t: T) {{ t.add() }}\nfn main() {{}}");
    let (_, ds) = checked_with(PRELUDE_SOURCE, &src);
    assert_eq!(errors(&src, &ds), [(AssignToImmutable, (2, 23))]);
}

/// GIVEN `interface K { fn add(mut self) }` and `struct S: K` with `fn add(mut self)`. WHEN the
/// program has `any K` as the type of a local, a parameter or a field, or `any L` with `interface
/// L: K {}`. THEN `sisu check` reports E0164 at the interface name after `any`. An interface that
/// cannot be `any` for a lasting reason as well gives that reason: the message is the one that
/// the program has with a plain `self` in place of the `mut self`.
#[test]
fn any_should_reject_an_interface_with_a_mut_self_requirement() {
    let s = "struct S: K { n: i64; fn add(mut self) { self.n += 1 } }\n";
    let rows: Vec<(&str, String, Vec<Wanted>)> = vec![
        (
            "a local",
            format!("{K_ADD}{s}fn main() {{ let a: any K = S(n: 1) }}"),
            vec![(CannotBeAny, (3, 24))],
        ),
        (
            "a parameter",
            format!("{K_ADD}{s}fn f(k: any K) {{}}\nfn main() {{}}"),
            vec![(CannotBeAny, (3, 13))],
        ),
        (
            "a field",
            format!("{K_ADD}{s}struct H {{ k: any K }}\nfn main() {{}}"),
            vec![(CannotBeAny, (3, 19))],
        ),
        (
            "an inherited requirement",
            format!("{K_ADD}{s}interface L: K {{}}\nfn f(l: any L) {{}}\nfn main() {{}}"),
            vec![(CannotBeAny, (4, 13))],
        ),
    ];
    error_rows(&rows);
    // The lasting reason wins: an associated type, another requirement, an inherited interface,
    // and an interface that inherits the `mut self` requirement.
    let lasting = [
        "interface K { type A; fn add(mut self) }\nfn f(k: any K) {}\nfn main() {}",
        "interface K { fn add(mut self); fn same(self, o: Self) -> bool }\nfn f(k: any K) {}\nfn main() {}",
        "interface L { fn same(self, o: Self) -> bool }\ninterface K: L { fn add(mut self) }\nfn f(k: any K) {}\nfn main() {}",
        "interface K { fn eq(self, o: Self) -> bool }\ninterface L: K { fn add(mut self) }\nfn f(l: any L) {}\nfn main() {}",
    ];
    for src in lasting {
        let plain = src.replace("mut self", "self");
        let (_, found) = parity_found(src);
        assert_eq!(found, parity_found(&plain).1, "{src}");
        assert_eq!(found.len(), 1, "{src}");
    }
}

/// The code, the span and whether a help is present, of each error of `src`.
fn error_shapes(src: &str) -> Vec<(Code, Span, bool)> {
    let (_, ds) = checked_with(PRELUDE_SOURCE, src);
    (ds.iter())
        .filter(|d| d.code().severity() == Severity::Error)
        .map(|d| (d.code(), d.span(), d.help().is_some()))
        .collect()
}

/// GIVEN `interface K { fn add(mut self) }`, and types that conform to it. WHEN a `?.` leads to a
/// call of `add`. THEN `sisu check` reports one E0062 from the `?.` to the name after it. A
/// struct or an enum in an optional root gets the help that replaces the root; a type parameter
/// or an associated type gets none. The fixture `E0062-mut-self-param` holds the texts.
#[test]
fn mut_self_call_after_a_safe_call_should_name_its_receiver() {
    let types = "struct P: K { n: i64; fn add(mut self) { self.n += 1 } }\n\
                 enum E: K { A; B; fn add(mut self) { self = E.B } }\n\
                 struct G<T>: K { v: T; fn add(mut self) {} }\n\
                 interface H { type A: K; fn get(self) -> A? }\n\
                 struct W<T: K> { t: T }\n";
    let rows = [
        (
            "a type parameter",
            "fn f<T: K>(mut o: T?) { o?.add() }",
            false,
        ),
        (
            "an associated type",
            "fn f<T: H>(t: T) { let mut o = t.get(); o?.add() }",
            false,
        ),
        (
            "a type parameter of a method",
            "struct S { n: i64; fn m<U: K>(self, mut o: U?) { o?.add() } }",
            false,
        ),
        (
            "two layers, one peeled",
            "fn f<T: K>(o: T??) { let mut p = o ?? None; p?.add() }",
            false,
        ),
        (
            "a field of a type parameter",
            "fn f<T: K>(mut o: W<T>?) { o?.t.add() }",
            true,
        ),
        ("a struct", "fn f(mut o: P?) { o?.add() }", true),
        ("an enum", "fn f(mut o: E?) { o?.add() }", true),
        (
            "a generic struct",
            "fn f(mut o: G<i64>?) { o?.add() }",
            true,
        ),
    ];
    let mut red = Vec::new();
    for (row, f, help) in rows {
        let src = format!("{K_ADD}{types}{f}\nfn main() {{}}");
        let start = src.find("?.").expect("the row should hold a `?.`");
        let name = (src[start + 2..].chars())
            .take_while(char::is_ascii_alphanumeric)
            .count();
        let wanted = [(MayBeNone, Span::new(start, start + 2 + name), help)];
        let found = error_shapes(&src);
        if found != wanted {
            red.push(format!("{row}: found {found:?}, wanted {wanted:?}"));
        }
    }
    assert!(red.is_empty(), "{}", red.join("\n"));
}

/// GIVEN `interface K { fn add(mut self) }`. WHEN `fn f(k: K)` writes it where a type belongs.
/// THEN `sisu check` reports E0147 at `K`, with a help. The fixture `E0147-mut-self` holds its
/// text, which offers a type parameter and no `any K`.
#[test]
fn interface_with_a_mut_self_requirement_as_a_type_should_offer_only_a_type_parameter() {
    let src = format!("{K_ADD}fn f(k: K) {{}}\nfn main() {{}}");
    let start = K_ADD.len() + "fn f(k: ".len();
    let wanted = [(InterfaceAsType, Span::new(start, start + 1), true)];
    assert_eq!(error_shapes(&src), wanted);
}

/// `T` with `step` and `next`, and the `u` that its loops compare with.
const STEP: &str = "struct T {\n    n: i64\n    fn step(mut self) { self.n += 1 }\n    fn next(mut self) -> i64 {\n        self.n += 1\n        self.n\n    }\n}\n";

/// GIVEN `struct T` with `fn step(mut self)` and `fn next(mut self) -> i64`, `let mut t = T(n: 0)`
/// and `let u = T(n: 3)`. WHEN the program has a `while` whose condition reads `t`. THEN `sisu
/// check` reports W0011 at the condition only when nothing in the loop changes `t`.
#[test]
fn mut_self_call_should_count_as_a_change_in_a_while() {
    let program = |lp: &str| {
        format!(
            "{STEP}fn main() {{\n    let mut t = T(n: 0)\n    let u = T(n: 3)\n    {lp}\n    print(t.n)\n    print(u.n)\n}}\n"
        )
    };
    let rows = [
        ("a call in the body", "while t != u { t.step() }", vec![]),
        (
            "no change",
            "while t != u { print(t.n) }",
            vec![(UnchangedLoopCondition, (12, 11))],
        ),
        (
            "a call with a result in the condition",
            "while t.next() < 3 {}",
            vec![],
        ),
    ];
    for (row, lp, wanted) in rows {
        codes_row(row, &program(lp), &wanted);
    }
}

/// GIVEN `Point` with `move_by`. WHEN `sisu check` runs on a program that calls it, or on a
/// `mut self` method that never changes `self`. THEN W0002 and W0014 follow the rule of a `mut`
/// argument. A call with no result is a write, and a call with a result is a read. An unused
/// `mut self` gets W0002 at `self`, with the safe fix that removes `mut `.
#[test]
fn unused_warnings_should_see_a_mut_self_call() {
    judge_unused();
    let program = |items: &str, body: &str| format!("{POINT}{items}fn main() {{\n    {body}\n}}\n");
    let rows = [
        (
            "a call with no result is a write",
            program("", "let mut p = Point(x: 1, y: 2)\n    p.move_by(1, 1)"),
            vec![(UnusedVariable, (10, 13))],
        ),
        (
            "the lost write of a copy",
            program(
                "",
                "let xs = [Point(x: 1, y: 2)]\n    let mut a = xs[0]\n    a.move_by(1, 1)",
            ),
            vec![(UnusedVariable, (11, 13))],
        ),
        (
            "a call with a result is a read",
            program(
                "extend Point {\n    fn next(mut self) -> i64 {\n        self.x += 1\n        self.x\n    }\n}\n",
                "let mut p = Point(x: 1, y: 2)\n    print(p.next())",
            ),
            vec![],
        ),
        (
            "an unused `mut self`",
            program(
                "struct T {\n    n: i64\n    fn f(mut self) { print(self.n) }\n}\n",
                "let mut t = T(n: 1)\n    t.f()\n    print(t.n)",
            ),
            vec![(NeverReassigned, (11, 14))],
        ),
        (
            "a struct's own `push`",
            program(
                "struct St {\n    xs: [i64]\n    fn push(mut self, x: i64) { self.xs.push(x) }\n}\n",
                "let mut s = St(xs: [])\n    s.push(1)\n    print(s.xs)",
            ),
            vec![],
        ),
    ];
    for (row, src, wanted) in rows {
        // `Point`'s fields and `move_by` go unused in some rows: W0012 and W0013 are not the row's.
        let found: Vec<_> = (all_codes(&src).into_iter())
            .filter(|&(code, _)| !matches!(code, Code::UnusedItem | UnusedMember))
            .collect();
        assert_eq!(found, wanted, "row `{row}`");
    }
    // The fix removes `mut ` and nothing else.
    let src = "struct T {\n    n: i64\n    fn f(mut self) { print(self.n) }\n}\nfn main() {\n    let mut t = T(n: 1)\n    t.f()\n    print(t.n)\n}\n";
    let (_, ds) = checked(src);
    let d = ds
        .iter()
        .find(|d| d.code() == NeverReassigned)
        .expect("W0002 should be reported");
    assert_eq!(at(src, d.span()), (3, 14), "{d:?}");
    let [fix] = d.fixes() else {
        panic!("W0002 should have one fix: {d:?}");
    };
    assert!(fix.safe(), "{d:?}");
    assert_eq!(fix.apply(src), src.replacen("(mut self)", "(self)", 1));
}

/// GIVEN a `mut self` method that never changes `self`. WHEN a call of it has a diagnostic that
/// exists only because the method takes `mut self`, before or after the method in the file. THEN
/// `sisu check` reports W0002 at `self` with an unsafe fix that removes `mut`, and keeps the
/// diagnostic of the call. With no such call, the fix stays safe.
#[test]
#[expect(clippy::too_many_lines, reason = "a table of rows")]
fn unused_mut_self_fix_should_be_unsafe_when_a_call_needs_the_mut() {
    judge_unused();
    let pile = "struct Pile {\n    n: i64\n    fn show(mut self) { print(self.n) }\n}\n";
    let bump = "struct T {\n    n: i64\n    fn bump(mut self) { print(self.n) }\n}\n";
    let rows = [
        (
            "E0171 at the call",
            "struct Pile {\n    n: i64\n    fn change(mut self, mut other: Pile) { other.n = 1 }\n}\nfn main() {\n    let mut pile = Pile(n: 0)\n    pile.change(mut pile)\n}\n".to_string(),
            Some(MutPlaceTwice),
            false,
        ),
        (
            "E0001 at the call",
            format!("{pile}fn main() {{\n    let pile = Pile(n: 0)\n    pile.show()\n}}\n"),
            Some(AssignToImmutable),
            false,
        ),
        (
            "E0001 at a call before the method",
            format!("fn main() {{\n    let pile = Pile(n: 0)\n    pile.show()\n}}\n{pile}"),
            Some(AssignToImmutable),
            false,
        ),
        (
            "no call needs the `mut`",
            format!("{pile}fn main() {{\n    let mut pile = Pile(n: 0)\n    pile.show()\n    print(pile.n)\n}}\n"),
            None,
            true,
        ),
        (
            "W0014 at a `let mut` that only the call writes",
            format!("{pile}fn main() {{\n    let mut pile = Pile(n: 0)\n    pile.show()\n}}\n"),
            Some(UnusedVariable),
            false,
        ),
        (
            "W0014 at a copy that only the call writes",
            "struct Pile {\n    n: i64\n    fn peek(mut self) { print(self.n) }\n}\nstruct W {\n    p: Pile\n}\nfn main() {\n    let mut w = W(p: Pile(n: 1))\n    w.p.peek()\n    let mut c = w\n    c.p.peek()\n}\n".to_string(),
            Some(UnusedVariable),
            false,
        ),
        (
            "E0050 at a generic call that takes the call",
            "struct B {\n    n: i64\n    fn none<U>(mut self) -> U? { None }\n}\nfn id<U>(u: U) -> U { u }\nfn main() {\n    let mut b = B(n: 1)\n    b.n = 2\n    let x: i64? = id(b.none())\n    print(x ?? 0)\n}\n".to_string(),
            Some(CannotInfer),
            false,
        ),
        (
            "E0050 at a generic call of a plain method that shares the name",
            "struct A {\n    n: i64\n    fn none(mut self) {}\n}\nstruct B {\n    n: i64\n    fn none<U>(self) -> U? { None }\n}\nfn id<U>(u: U) -> U { u }\nfn main() {\n    let mut a = A(n: 1)\n    a.none()\n    print(a.n)\n    let b = B(n: 1)\n    let x: i64? = id(b.none())\n    print(x ?? 0)\n}\n".to_string(),
            Some(CannotInfer),
            false,
        ),
        (
            "E0021 on a temporary",
            format!("{bump}fn main() {{\n    T(n: 0).bump()\n}}\n"),
            Some(AssignToExpression),
            false,
        ),
        (
            "E0062 at a `?.` call",
            format!(
                "{bump}fn main() {{\n    let mut o: T? = Some(T(n: 0))\n    o?.bump()\n}}\n"
            ),
            Some(MayBeNone),
            false,
        ),
        (
            "E0001 at a `?.` call on a `let`",
            format!("{bump}fn main() {{\n    let o: T? = Some(T(n: 0))\n    o?.bump()\n}}\n"),
            Some(AssignToImmutable),
            false,
        ),
        (
            "E0001 at a call on a plain `self`",
            format!(
                "{bump}extend T {{\n    fn look(self) {{ self.bump() }}\n}}\nfn main() {{\n    let t = T(n: 0)\n    t.look()\n}}\n"
            ),
            Some(AssignToImmutable),
            false,
        ),
        (
            "E0001 at a call of an enum's method",
            "enum E {\n    A\n    B\n    fn flip(mut self) { print(1) }\n}\nfn main() {\n    let e = E.A\n    e.flip()\n}\n".to_string(),
            Some(AssignToImmutable),
            false,
        ),
        (
            "E0001 at a call of a method of an `extend` block",
            "struct T {\n    n: i64\n}\nextend T {\n    fn bump(mut self) { print(self.n) }\n}\nfn main() {\n    let t = T(n: 0)\n    t.bump()\n}\n".to_string(),
            Some(AssignToImmutable),
            false,
        ),
        (
            "W0014 at a local that a write also changes",
            format!(
                "{bump}fn main() {{\n    let xs = [T(n: 1)]\n    let mut a = xs[0]\n    a.n = 2\n    a.bump()\n}}\n"
            ),
            Some(UnusedVariable),
            false,
        ),
        (
            "W0014 at a local that another `mut self` call changes",
            "struct T {\n    n: i64\n    fn bump(mut self) { print(self.n) }\n    fn bop(mut self) { self.n = 1 }\n}\nfn main() {\n    let mut t = T(n: 0)\n    t.bump()\n    t.bop()\n}\n".to_string(),
            Some(UnusedVariable),
            false,
        ),
    ];
    for (row, src, kept, safe) in rows {
        let (_, ds) = checked(&src);
        let d = ds
            .iter()
            .find(|d| {
                d.code() == NeverReassigned && &src[d.span().start()..d.span().end()] == "self"
            })
            .unwrap_or_else(|| panic!("row `{row}`: W0002 should be at `self`: {ds:?}"));
        let [fix] = d.fixes() else {
            panic!("row `{row}`: W0002 should have one fix: {d:?}");
        };
        assert_eq!(fix.safe(), safe, "row `{row}`: {d:?}");
        assert_eq!(
            fix.apply(&src),
            src.replacen("(mut self", "(self", 1),
            "row `{row}`"
        );
        if let Some(code) = kept {
            assert!(ds.iter().any(|d| d.code() == code), "row `{row}`: {ds:?}");
        }
    }
}

/// GIVEN `extend<T> G<T>: K { fn add(mut self) {} }`, a `struct T` whose `f` does `self = T(n: 9)`,
/// and one whose `f` loops with `self = T(n: self.n + 1)`. WHEN `sisu check` runs on each. THEN
/// no row has W0002, and the loop has no W0011: a write of `self` counts as a change.
#[test]
fn mut_self_should_count_a_write_of_self_and_a_requirement() {
    judge_unused();
    let rows = [
        (
            "a requirement keeps the `mut`",
            "interface K { fn add(mut self) }\nstruct G<T> { v: T }\nextend<T> G<T>: K { fn add(mut self) {} }\nfn main() {\n    let mut g = G(v: 1)\n    g.add()\n    print(g.v)\n}\n",
        ),
        (
            "a whole write of `self`",
            "struct T {\n    n: i64\n    fn f(mut self) { self = T(n: 9) }\n}\nfn main() {\n    let mut t = T(n: 1)\n    t.f()\n    print(t.n)\n}\n",
        ),
        (
            "a loop that writes `self`",
            "struct T {\n    n: i64\n    fn f(mut self) {\n        while self.n < 3 { self = T(n: self.n + 1) }\n    }\n}\nfn main() {\n    let mut t = T(n: 1)\n    t.f()\n    print(t.n)\n}\n",
        ),
    ];
    for (row, src) in rows {
        codes_row(row, src, &[]);
    }
}

/// GIVEN `struct S { p: Point; k: i64 }` and `let mut s = S(p: Point(x: 1, y: 2), k: 3)`. WHEN
/// `main` calls `s.p.move_by(1, 1)` and prints `s.k`, and nothing else reads `p`. THEN `sisu check`
/// reports no W0013 at `p`, as `bump(mut s.p)` has none.
#[test]
fn mut_self_call_on_a_field_should_read_the_field() {
    judge_unused();
    let src = format!(
        "{POINT}struct S {{\n    p: Point\n    k: i64\n}}\nfn main() {{\n    let mut s = S(p: Point(x: 1, y: 2), k: 3)\n    s.p.move_by(1, 1)\n    print(s.k)\n    let q = Point(x: 1, y: 2)\n    print(q.x + q.y)\n}}\n"
    );
    codes(&src, &[]);
}

/// GIVEN `struct P` with `fn bump(mut self)`, and a `struct T` with the fields `o: P?` and `l: L`.
/// `L` has `o: P?`. WHEN a `mut self` method of `T` calls `bump`, or writes a field of `P`,
/// through a `?.` on a part of `self`. The same holds in an `if` or a `while`. THEN `sisu check`
/// reports one E0062 at the `?.` and no W0002. The call counts as a change of `self`.
#[test]
fn mut_self_call_through_an_optional_part_of_self_should_report_only_e0062() {
    let rows = [
        ("a field", "self.o?.bump()"),
        ("a field of a field", "self.l.o?.bump()"),
        ("in an `if`", "if self.n > 0 { self.o?.bump() }"),
        ("in a `while`", "while self.n > 0 { self.o?.bump(); break }"),
        ("a write of a field", "self.o?.x = 2"),
        ("a write in an `if`", "if self.n > 0 { self.o?.x = 2 }"),
    ];
    for (row, body) in rows {
        let src = format!(
            "struct P {{\n    x: i64\n    fn bump(mut self) {{ self.x += 1 }}\n}}\nstruct L {{\n    o: P?\n}}\n\
             struct T {{\n    n: i64\n    o: P?\n    l: L\n    fn f(mut self) {{ {body} }}\n}}\nfn main() {{}}\n"
        );
        assert_eq!(
            all_codes(&src),
            [(MayBeNone, on(&src, 12, "?."))],
            "row `{row}`"
        );
    }
}

/// GIVEN `struct T` whose `mut self` method `f` has the body `todo`. WHEN `sisu check` runs on it.
/// THEN it reports W0003 for the `todo` and no W0002: the body is a stub, as for a `mut` parameter.
#[test]
fn mut_self_method_with_a_todo_body_should_get_no_unused_warning() {
    let src = "struct T {\n    n: i64\n    fn f(mut self) { todo }\n}\nfn main() {\n    let mut t = T(n: 1)\n    t.f()\n    print(t.n)\n}\n";
    let found: Vec<_> = all_codes(src)
        .into_iter()
        .map(|(code, _)| code.as_str())
        .collect();
    assert_eq!(found, ["W0003"]);
}

/// GIVEN `extend Q { fn g(mut self: Q) { print(self.x) } }`. WHEN `sisu check` runs on it. THEN it
/// reports E0011 at the `:` and no W0002: the annotation is the one mistake.
#[test]
fn mut_self_with_a_type_should_report_only_the_expected_paren() {
    let src = "struct Q { x: i64 }\nextend Q {\n    fn g(mut self: Q) { print(self.x) }\n}\nfn main() {}\n";
    assert_eq!(all_codes(src), [(Expected, on(src, 3, ":"))]);
}

/// `T` with the `mut self` method `bump` and the plain `self` method `peek`, then `H`, which holds
/// a `T`, in six lines.
const BUMP: &str = "struct T {\n    n: i64\n    fn bump(mut self, k: i64) { self.n += k }\n    fn peek(self, k: i64) { print(self.n + k) }\n}\nstruct H { t: T }\n";

/// `BUMP`, then a `main` with `body` in it.
fn bump_main(body: &str) -> String {
    format!("{BUMP}fn main() {{\n{body}\n}}\n")
}

/// E0070 and E0120 have a fix that calls the method on its first argument. It is safe only when
/// the receiver can change or the method takes plain `self`. Each row: the program, the code, and
/// whether the fix is safe. A safe fix gives a program with no error.
#[test]
#[expect(clippy::too_many_lines, reason = "a table of rows")]
fn call_as_method_fix_should_be_safe_only_when_the_receiver_can_change() {
    let in_f = |s: &str| format!("{BUMP}fn f({s}\nfn main() {{}}\n");
    let mut rows: Vec<(&str, String, Code, bool)> = vec![
        (
            "E0120, `let mut`",
            bump_main("    let mut t = T(n: 1)\n    T.bump(t, 1)\n    print(t.n)"),
            Code::MemberKind,
            true,
        ),
        (
            "E0120, `let`",
            bump_main("    let t = T(n: 1)\n    T.bump(t, 1)\n    print(t.n)"),
            Code::MemberKind,
            false,
        ),
        (
            "E0120, plain parameter",
            in_f("t: T) { T.bump(t, 1) }"),
            Code::MemberKind,
            false,
        ),
        (
            "E0120, `mut` parameter",
            in_f("mut t: T) { T.bump(t, 1) }"),
            Code::MemberKind,
            true,
        ),
        (
            "E0120, field of `let mut`",
            bump_main("    let mut h = H(t: T(n: 1))\n    T.bump(h.t, 1)\n    print(h.t.n)"),
            Code::MemberKind,
            true,
        ),
        (
            "E0120, field of `let`",
            bump_main("    let h = H(t: T(n: 1))\n    T.bump(h.t, 1)\n    print(h.t.n)"),
            Code::MemberKind,
            false,
        ),
        (
            "E0120, element of `let mut`",
            bump_main("    let mut ts = [T(n: 1)]\n    T.bump(ts[0], 1)\n    print(ts[0].n)"),
            Code::MemberKind,
            true,
        ),
        (
            "E0120, plain `self` method, `let` (passes before the fix)",
            bump_main("    let t = T(n: 1)\n    T.peek(t, 1)"),
            Code::MemberKind,
            true,
        ),
        (
            "E0070, `let mut`",
            bump_main("    let mut t = T(n: 1)\n    bump(t, 1)\n    print(t.n)"),
            Code::MethodAsFunction,
            true,
        ),
        (
            "E0070, `let`",
            bump_main("    let t = T(n: 1)\n    bump(t, 1)\n    print(t.n)"),
            Code::MethodAsFunction,
            false,
        ),
        (
            "E0070, plain parameter",
            in_f("t: T) { bump(t, 1) }"),
            Code::MethodAsFunction,
            false,
        ),
        (
            "E0070, `mut` parameter",
            in_f("mut t: T) { bump(t, 1) }"),
            Code::MethodAsFunction,
            true,
        ),
        (
            "E0070, plain `self` method, `let` (passes before the fix)",
            bump_main("    let t = T(n: 1)\n    peek(t, 1)"),
            Code::MethodAsFunction,
            true,
        ),
    ];
    let flip = |s: &str| {
        format!(
            "enum E {{\n    A\n    fn flip(mut self) {{ self = E.A }}\n    {s}\n}}\nfn main() {{}}\n"
        )
    };
    rows.push((
        "E0120, `self` of an enum `mut self` method",
        flip("fn h(mut self) { E.flip(self) }"),
        Code::MemberKind,
        true,
    ));
    rows.push((
        "E0120, `self` of an enum plain `self` method",
        flip("fn h(self) { E.flip(self) }"),
        Code::MemberKind,
        false,
    ));
    let mut red = Vec::new();
    for (row, src, code, safe) in &rows {
        let (_, ds) = checked(src);
        let Some(d) = ds.iter().find(|d| d.code() == *code) else {
            red.push(format!("row `{row}`: no {code:?} in {ds:?}"));
            continue;
        };
        let Some(fix) = d.fixes().first() else {
            red.push(format!("row `{row}`: no fix"));
            continue;
        };
        if fix.safe() != *safe {
            red.push(format!("row `{row}`: safe is {}", fix.safe()));
            continue;
        }
        if *safe {
            let fixed = fix.apply(src);
            let left = errors(&fixed, &diags(&fixed));
            if !left.is_empty() {
                red.push(format!("row `{row}`: the fixed program gives {left:?}"));
            }
        }
    }
    assert!(
        red.is_empty(),
        "{} red rows:\n{}",
        red.len(),
        red.join("\n")
    );
}

/// `T` with the `mut self` method `bump`. A bare `bump(1)` is in a plain `self` method on line 4,
/// and in a `mut self` method on line 5.
const BARE_BUMP: &str = "struct T {\n    n: i64\n    fn bump(mut self, k: i64) { self.n += k }\n    fn read(self) { bump(1) }\n    fn write(mut self) { bump(1) }\n}\nfn main() {}\n";

/// The `self` form of E0070, a bare `bump(1)` in a method of `T`: its fix is safe only in a
/// `mut self` method. In that method, the failed call counts as a write of `self`, so W0002 is
/// not reported.
#[test]
fn self_form_of_call_as_method_should_be_safe_only_in_a_mut_self_method() {
    let src = BARE_BUMP;
    let (_, ds) = checked(src);
    let found: Vec<_> = (ds.iter())
        .map(|d| {
            (
                d.code(),
                at(src, d.span()),
                d.fixes().first().map(Fix::safe),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (Code::MethodAsFunction, on(src, 4, "bump"), Some(false)),
            (Code::MethodAsFunction, on(src, 5, "bump"), Some(true)),
        ]
    );
}

/// A failed call of a `mut self` method as a function or through its type would change its
/// receiver. So W0002 does not report a `let mut` or a `mut` parameter at the root of it.
/// A plain `self` method changes nothing, so W0002 stays. Each row: the program and the codes
/// it gives after `BUMP`.
#[test]
fn call_as_method_error_should_count_as_a_write_of_a_mut_self_receiver() {
    let in_f = |s: &str| format!("{BUMP}fn f({s}\nfn main() {{}}\n");
    let rows = [
        (
            bump_main("    let mut t = T(n: 1)\n    T.bump(t, 1)\n    print(t.n)"),
            vec![Code::MemberKind],
        ),
        (
            bump_main("    let mut t = T(n: 1)\n    bump(t, 1)\n    print(t.n)"),
            vec![Code::MethodAsFunction],
        ),
        (
            bump_main("    let mut h = H(t: T(n: 1))\n    T.bump(h.t, 1)\n    print(h.t.n)"),
            vec![Code::MemberKind],
        ),
        (
            bump_main("    let mut ts = [T(n: 1)]\n    T.bump(ts[0], 1)\n    print(ts[0].n)"),
            vec![Code::MemberKind],
        ),
        (in_f("mut t: T) { T.bump(t, 1) }"), vec![Code::MemberKind]),
        (
            in_f("mut t: T) { bump(t, 1) }"),
            vec![Code::MethodAsFunction],
        ),
        (
            "enum E {\n    A\n    fn flip(mut self) { self = E.A }\n    fn h(mut self) { E.flip(self) }\n}\nfn main() {}\n".to_string(),
            vec![Code::MemberKind],
        ),
        (
            bump_main("    let mut t = T(n: 1)\n    T.peek(t, 1)"),
            vec![NeverReassigned, Code::MemberKind],
        ),
        (
            bump_main("    let mut t = T(n: 1)\n    peek(t, 1)"),
            vec![NeverReassigned, Code::MethodAsFunction],
        ),
    ];
    let mut red = Vec::new();
    for (src, wanted) in rows {
        let found: Vec<_> = (all_codes(&src).into_iter())
            .filter(|&(code, _)| !matches!(code, Code::UnusedItem | UnusedMember))
            .map(|(code, _)| code)
            .collect();
        if found != wanted {
            red.push(format!("row `{src}`: found {found:?}"));
        }
    }
    assert!(
        red.is_empty(),
        "{} red rows:\n{}",
        red.len(),
        red.join("\n")
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "a table of duplicate conformance cases"
)]
fn duplicate_conformance_methods_should_respect_requirement_self_and_mut_rules() {
    let decls = "interface R { fn bump(self) }\n\
                 interface SubR: R { fn extra(self) }\n\
                 interface Rm { fn bump(mut self) }\n\
                 interface Rp { fn f(self, mut k: i64) }\n\
                 interface Rv { fn f(self) }\n\
                 interface Gen<T> { fn g(self, x: T) }\n\
                 struct S { n: i64 }\n\
                 fn take(mut s: S) { s.n = 2 }\n";
    let plain_rows = [
        (
            "duplicate block, field write",
            format!(
                "{decls}extend S: R {{ fn bump(self) {{ print(self.n) }} }}\n\
                 extend S: R {{ fn bump(self) {{ self.n += 1 }} }}\n\
                 fn main() {{ let s = S(n: 1); s.bump() }}"
            ),
            AssignToImmutable,
        ),
        (
            "duplicate block, whole self write",
            format!(
                "{decls}extend S: R {{ fn bump(self) {{ print(self.n) }} }}\n\
                 extend S: R {{ fn bump(self) {{ self = S(n: 1) }} }}\n\
                 fn main() {{ let s = S(n: 1); s.bump() }}"
            ),
            AssignToSelf,
        ),
        (
            "duplicate block, mut argument on self",
            format!(
                "{decls}extend S: R {{ fn bump(self) {{ print(self.n) }} }}\n\
                 extend S: R {{ fn bump(self) {{ take(mut self) }} }}\n\
                 fn main() {{ let s = S(n: 1); s.bump() }}"
            ),
            MutArgumentNotPlace,
        ),
        (
            "duplicate block clashing with listed struct",
            format!(
                "{decls}struct S2: R {{ n: i64; fn bump(self) {{ print(self.n) }} }}\n\
                 extend S2: R {{ fn bump(self) {{ self.n += 1 }} }}\n\
                 fn main() {{ let s = S2(n: 1); s.bump() }}"
            ),
            AssignToImmutable,
        ),
        (
            "duplicate block clashing with inherited conformance from SubR",
            format!(
                "{decls}struct S4: SubR {{ n: i64; fn bump(self) {{ print(self.n) }}; fn extra(self) {{}} }}\n\
                 extend S4: R {{ fn bump(self) {{ self.n += 1 }} }}\n\
                 fn main() {{ let s = S4(n: 1); s.bump() }}"
            ),
            AssignToImmutable,
        ),
        (
            "duplicate SubR block requirement",
            format!(
                "{decls}extend S: R {{ fn bump(self) {{ print(self.n) }} }}\n\
                 extend S: SubR {{ fn extra(self) {{}} }}\n\
                 extend S: SubR {{ fn extra(self) {{ self.n += 1 }} }}\n\
                 fn main() {{ let s = S(n: 1); s.extra() }}"
            ),
            AssignToImmutable,
        ),
        (
            "non-duplicate block, unchanged",
            format!(
                "{decls}struct T {{ n: i64 }}\n\
                 extend T: R {{ fn bump(self) {{ self.n += 1 }} }}\n\
                 fn main() {{ let t = T(n: 1); t.bump() }}"
            ),
            AssignToImmutable,
        ),
        (
            "duplicate listed conformance on struct, unchanged",
            format!(
                "{decls}struct S3: R, R {{ n: i64; fn bump(self) {{ self.n += 1 }} }}\n\
                 fn main() {{ let s = S3(n: 1); s.bump() }}"
            ),
            AssignToImmutable,
        ),
    ];
    for (row, src, code) in plain_rows {
        let (_, ds) = checked(&src);
        let found = ds
            .iter()
            .find(|d| d.code() == code)
            .unwrap_or_else(|| panic!("row `{row}`: missing {code:?} in {ds:?}"));
        assert!(found.fixes().is_empty(), "row `{row}`: {found:?}");
        assert!(found.help().is_some(), "row `{row}`: {found:?}");
    }

    let no_w0002_rows = [
        (
            "duplicate mut self requirement with empty body",
            format!(
                "{decls}extend S: Rm {{ fn bump(mut self) {{}} }}\n\
                 extend S: Rm {{ fn bump(mut self) {{}} }}\n\
                 fn main() {{ let mut s = S(n: 1); s.bump(); print(s.n) }}"
            ),
        ),
        (
            "duplicate plain requirement with mut self implementation",
            format!(
                "{decls}extend S: R {{ fn bump(self) {{ print(self.n) }} }}\n\
                 extend S: R {{ fn bump(mut self) {{ print(self.n) }} }}\n\
                 fn main() {{ let s = S(n: 1); s.bump() }}"
            ),
        ),
        (
            "duplicate requirement with unused mut parameter",
            format!(
                "{decls}extend S: Rp {{ fn f(self, mut k: i64) {{ print(k) }} }}\n\
                 extend S: Rp {{ fn f(self, mut k: i64) {{ print(k) }} }}\n\
                 fn main() {{ let s = S(n: 1); s.f(1) }}"
            ),
        ),
    ];
    for (row, src) in no_w0002_rows {
        let (_, ds) = checked(&src);
        assert!(
            !ds.iter().any(|d| d.code() == NeverReassigned),
            "row `{row}`: unexpected W0002 in {ds:?}"
        );
    }

    let void_return = format!(
        "{decls}extend S: Rv {{ fn f(self) {{}} }}\n\
         extend S: Rv {{ fn f(self) {{ return 1 }} }}\n\
         fn main() {{ let s = S(n: 1); s.f() }}"
    );
    let (_, ds) = checked(&void_return);
    let ret = ds
        .iter()
        .find(|d| d.code() == UnexpectedReturnValue)
        .unwrap_or_else(|| panic!("void return: missing E0037 in {ds:?}"));
    assert!(ret.fixes().is_empty(), "void return: {ret:?}");

    let edge_rows = [
        (
            "broken method in duplicate block",
            format!(
                "{decls}extend S: R {{ fn bump(self) {{ print(self.n) }} }}\n\
                 extend S: R {{ fn bump(self) {{ let = 1 }} }}\n\
                 fn main() {{}}"
            ),
            ConformsTwice,
        ),
        (
            "broken method header in duplicate block",
            format!(
                "{decls}extend S: R {{ fn bump(self) {{ print(self.n) }} }}\n\
                 extend S: R {{ fn bump(self: i64) {{ print(1) }} }}\n\
                 fn main() {{}}"
            ),
            ConformsTwice,
        ),
        (
            "bad interface bound in duplicate block",
            format!(
                "{decls}extend S: Gen<i64> {{ fn g(self, x: i64) {{ print(x) }} }}\n\
                 extend S: Gen<i64, i64> {{ fn g(self, x: i64) {{ print(x) }} }}\n\
                 fn main() {{}}"
            ),
            TypeArguments,
        ),
    ];
    let empty_prelude = prelude_program("");
    for (row, src, code) in edge_rows {
        let lexed = crate::lexer::lex(&src).expect("the source should lex");
        let parsed = crate::parser::parse(lexed, false);
        let (_, ds) = check(&parsed.program, &empty_prelude, &src, &parsed.diagnostics);
        assert!(
            ds.iter().any(|d| d.code() == code),
            "row `{row}`: missing {code:?} in {ds:?}"
        );
    }
}

#[test]
#[expect(clippy::too_many_lines, reason = "a table of chain-index forms")]
fn safe_chain_index_forms_should_report_one_consistent_error() {
    judge_unused();
    let decls = "interface Rm { fn set(mut self) }\n\
                 interface R { fn act(self) }\n\
                 struct P: Rm {\n\
                     x: i64\n\
                     fn set(mut self) { self.x = 1 }\n\
                     fn show(self) { print(self.x) }\n\
                 }\n\
                 struct Collider { x: i64; fn show(mut self) { self.x = 2 } }\n\
                 struct Sp { x: i64; fn push(self, k: i64) -> i64 { self.x + k } }\n\
                 struct Mp { n: i64; fn push(mut self, k: i64) { self.n += k } }\n\
                 enum D { N; S; fn flip(mut self) { self = D.S } }\n\
                 struct L<T: Rm> { ps: [P]; qs: [[i64]]; qss: [[[i64]]]; sp: [Sp]; mp: [Mp]; ds: [D]; ts: [T] }\n\
                 struct Lo { ps: [P]? }\n\
                 fn id<U>(u: U) -> U { u }\n";
    let main_call = "\nfn main() { let mut o: L<P>? = None; f(mut o); print(o != None) }";
    let mut_o_rows = [
        (
            "field assign, unchanged",
            "fn f(mut o: L<P>?) { o?.ps[0].x = 2 }",
            "?.ps",
            vec![MayBeNone],
        ),
        (
            "compound field assign, unchanged",
            "fn f(mut o: L<P>?) { o?.ps[0].x += 1 }",
            "?.ps",
            vec![MayBeNone],
        ),
        (
            "mut self call",
            "fn f(mut o: L<P>?) { o?.ps[0].set() }",
            "?.ps",
            vec![MayBeNone],
        ),
        (
            "enum mut self call",
            "fn f(mut o: L<P>?) { o?.ds[0].flip() }",
            "?.ds",
            vec![MayBeNone],
        ),
        (
            "generic param mut self call",
            "fn g<T: Rm>(mut o: L<T>?) { o?.ts[0].set() }",
            "?.ts",
            vec![MayBeNone],
        ),
        (
            "plain self call with mut o, unchanged",
            "fn f(mut o: L<P>?) { o?.ps[0].show() }",
            "o?.ps",
            vec![NeverReassigned, MayBeNone],
        ),
        (
            "array push",
            "fn f(mut o: L<P>?) { o?.qs[0].push(2) }",
            "?.qs",
            vec![MayBeNone],
        ),
        (
            "nested array push",
            "fn f(mut o: L<P>?) { o?.qss[0][0].push(2) }",
            "?.qss",
            vec![MayBeNone],
        ),
        (
            "plain struct push, unchanged",
            "fn f(mut o: L<P>?) { let k = o?.sp[0].push(2); print(k) }",
            "o?.sp",
            vec![MayBeNone],
        ),
        (
            "mut self struct push",
            "fn f(mut o: L<P>?) { o?.mp[0].push(2) }",
            "?.mp",
            vec![MayBeNone],
        ),
        (
            "optional array field mut self call",
            "fn f(mut o: Lo?) { o?.ps[0].set() }",
            "?.ps",
            vec![MayBeNone],
        ),
        (
            "generic context mut self call",
            "fn f(mut o: L<P>?) { id(o?.ps[0].set()) }",
            "?.ps",
            vec![MayBeNone],
        ),
        (
            "generic context plain self call, unchanged",
            "fn f(mut o: L<P>?) { id(o?.ps[0].show()) }",
            "o?.ps",
            vec![NeverReassigned, MayBeNone],
        ),
    ];
    for (row, body, needle, want) in mut_o_rows {
        let src = if body.contains("fn g<") {
            format!(
                "{decls}{body}\nfn main() {{ let mut o: L<P>? = None; g(mut o); print(o != None) }}"
            )
        } else if body.contains("Lo?") {
            format!(
                "{decls}{body}\nfn main() {{ let mut o: Lo? = None; f(mut o); print(o != None) }}"
            )
        } else {
            format!("{decls}{body}{main_call}")
        };
        let (_, ds) = checked(&src);
        let got: Vec<Code> = ds.iter().map(Diagnostic::code).collect();
        assert_eq!(got, want, "row `{row}`: {ds:?}");
        let err = ds
            .iter()
            .find(|d| d.code() == MayBeNone)
            .expect("row should have E0062");
        let line = src.lines().count() - 1;
        assert_eq!(at(&src, err.span()), on(&src, line, needle), "row `{row}`");
    }

    // Colliding `show` name in `mut_self_names` (`Collider`) must not affect `o?.ps[0].show()`.
    let colliding = format!(
        "{decls}fn use_col(mut c: Collider) {{ c.show() }}\n\
         fn f(mut o: L<P>?) {{ o?.ps[0].show() }}\n\
         fn main() {{\n\
             let mut c = Collider(x: 1); use_col(mut c); print(c.x)\n\
             let mut o: L<P>? = None; f(mut o); print(o != None)\n\
         }}"
    );
    let (_, ds) = checked(&colliding);
    assert_eq!(
        ds.iter().map(Diagnostic::code).collect::<Vec<_>>(),
        vec![NeverReassigned, MayBeNone]
    );
    let line = colliding.lines().count() - 4;
    let err = ds
        .iter()
        .find(|d| d.code() == MayBeNone)
        .expect("colliding should have E0062");
    assert_eq!(at(&colliding, err.span()), on(&colliding, line, "o?.ps"));

    // Immutable parameter roots give 1 E0001.
    let immut_rows = [
        (
            "immutable param mut self call",
            format!("{decls}fn f(o: L<P>?) {{ o?.ps[0].set() }}\nfn main() {{ f(None) }}"),
        ),
        (
            "immutable param array push",
            format!("{decls}fn f(o: L<P>?) {{ o?.qs[0].push(2) }}\nfn main() {{ f(None) }}"),
        ),
    ];
    for (row, src) in immut_rows {
        let (_, ds) = checked(&src);
        assert_eq!(
            ds.iter().map(Diagnostic::code).collect::<Vec<_>>(),
            vec![AssignToImmutable],
            "row `{row}`: {ds:?}"
        );
    }

    // `self.o?.ps[0].set()` in plain-self method vs duplicate conformance.
    let plain_self_src = format!(
        "{decls}struct H {{ o: L<P>?; fn act(self) {{ self.o?.ps[0].set() }} }}\n\
         fn main() {{ H(o: None).act() }}"
    );
    let (_, ds) = checked(&plain_self_src);
    assert_eq!(
        ds.iter().map(Diagnostic::code).collect::<Vec<_>>(),
        vec![AssignToImmutable]
    );
    assert!(
        !ds[0].fixes().is_empty(),
        "plain self method should offer self_take fix: {ds:?}"
    );

    let dup_self_src = format!(
        "{decls}struct H {{ o: L<P>? }}\n\
         extend H: R {{ fn act(self) {{ print(self.o != None) }} }}\n\
         extend H: R {{ fn act(self) {{ self.o?.ps[0].set() }} }}\n\
         fn main() {{ H(o: None).act() }}"
    );
    let (_, ds) = checked(&dup_self_src);
    let e0001 = ds
        .iter()
        .find(|d| d.code() == AssignToImmutable)
        .unwrap_or_else(|| panic!("dup self should have E0001: {ds:?}"));
    assert!(
        e0001.fixes().is_empty(),
        "duplicate conformance should have no self_take fix: {e0001:?}"
    );
    assert!(e0001.help().is_some());
}

/// GIVEN `struct P` with `fn set(mut self)`, `interface Rp` with plain `fn set(self)`,
/// `interface Rm` with `fn set(mut self)`, and `struct Lg<T> { ts: [T] }`. WHEN a function calls
/// `o?.ts[0].set()` on `mut o: Lg<T>?` or `Lg<T.Item>?`. THEN plain or missing bounds report
/// E0062 at `o?.ts` with W0002 on `o`, while `mut self` bounds stay at `?.ts`.
#[test]
fn chain_mut_self_call_should_check_required_bounds_on_type_parameters() {
    judge_unused();
    let decls = "interface Rp { fn set(self) }\n\
                 interface Rm { fn set(mut self) }\n\
                 interface HasP { type Item: Rp }\n\
                 interface HasM { type Item: Rm }\n\
                 struct P: Rm { x: i64; fn set(mut self) { self.x = 1 } }\n\
                 struct Q: Rp { x: i64; fn set(self) { print(self.x) } }\n\
                 struct Mp: HasP { type Item = Q }\n\
                 struct Mm: HasM { type Item = P }\n\
                 struct Lg<T> { ts: [T] }\n";
    let main_end = "\nfn main() {}";
    let unchanged = [
        (
            "mut self bound `Rm`, unchanged",
            format!("{decls}fn g<T: Rm>(mut o: Lg<T>?) {{ o?.ts[0].set() }}{main_end}"),
            vec![MayBeNone],
            "?.ts",
        ),
        (
            "associated type `T.Item` with `Rm`, unchanged",
            format!(
                "{decls}fn g<T: HasM>(mut o: Lg<T.Item>?) {{ o?.ts[0].set() }}\nfn main() {{ let mut o: Lg<P>? = None; g<Mm>(mut o); print(o != None) }}"
            ),
            vec![MayBeNone],
            "?.ts",
        ),
    ];
    for (row, src, want, needle) in unchanged {
        let (_, ds) = checked(&src);
        let got: Vec<Code> = ds.iter().map(Diagnostic::code).collect();
        assert_eq!(got, want, "row `{row}`: {ds:?}");
        let err = ds
            .iter()
            .find(|d| d.code() == MayBeNone)
            .expect("row should have E0062");
        let line = src.lines().count() - 1;
        assert_eq!(at(&src, err.span()), on(&src, line, needle), "row `{row}`");
    }

    let rows = [
        (
            "plain interface bound `Rp`",
            format!("{decls}fn g<T: Rp>(mut o: Lg<T>?) {{ o?.ts[0].set() }}{main_end}"),
            vec![NeverReassigned, MayBeNone],
            "o?.ts",
        ),
        (
            "unbounded type parameter `T`",
            format!("{decls}fn g<T>(mut o: Lg<T>?) {{ o?.ts[0].set() }}{main_end}"),
            vec![NeverReassigned, MayBeNone, NoMember],
            "o?.ts",
        ),
        (
            "mixed bounds `Rp & Rm`",
            format!("{decls}fn g<T: Rp & Rm>(mut o: Lg<T>?) {{ o?.ts[0].set() }}{main_end}"),
            vec![NeverReassigned, MayBeNone, Ambiguous],
            "o?.ts",
        ),
        (
            "associated type `T.Item` with plain `Rp`",
            format!(
                "{decls}fn g<T: HasP>(mut o: Lg<T.Item>?) {{ o?.ts[0].set() }}\nfn main() {{ let mut o: Lg<Q>? = None; g<Mp>(mut o); print(o != None) }}"
            ),
            vec![NeverReassigned, MayBeNone],
            "o?.ts",
        ),
    ];
    for (row, src, want, needle) in rows {
        let (_, ds) = checked(&src);
        let got: Vec<Code> = ds.iter().map(Diagnostic::code).collect();
        assert_eq!(got, want, "row `{row}`: {ds:?}");
        let err = ds
            .iter()
            .find(|d| d.code() == MayBeNone)
            .expect("row should have E0062");
        let line = src.lines().count() - 1;
        assert_eq!(at(&src, err.span()), on(&src, line, needle), "row `{row}`");
    }
}

/// GIVEN nested arrays of structs, enums, and generic elements on an optional struct. WHEN a
/// function writes a field or calls a `mut self` method after indexing every layer. THEN `sisu
/// check` reports E0062 at `?.<field>` with no W0002, while an unindexed array or plain method
/// keeps `o?.<field>` as the E0062 target.
#[test]
fn nested_array_after_safe_chain_should_report_chain_error_on_leaf_element() {
    judge_unused();
    let decls = "interface Rm { fn set(mut self) }\n\
                 struct P: Rm {\n\
                     x: i64\n\
                     fn set(mut self) { self.x = 1 }\n\
                     fn show(self) { print(self.x) }\n\
                 }\n\
                 enum D { N; S; fn flip(mut self) { self = D.S } }\n\
                 struct L<T: Rm> { pss: [[P]]; psss: [[[P]]]; dss: [[D]]; tss: [[T]] }\n";
    let main_p = "\nfn main() { let mut o: L<P>? = None; f(mut o); print(o != None) }";
    let unchanged = [
        (
            "1D index into 2D struct array, unchanged",
            format!("{decls}fn f(mut o: L<P>?) {{ o?.pss[0].x = 2 }}{main_p}"),
            vec![MayBeNone, NoMember],
            "o?.pss",
        ),
        (
            "2D struct plain self call, unchanged",
            format!("{decls}fn f(mut o: L<P>?) {{ o?.pss[0][0].show() }}{main_p}"),
            vec![NeverReassigned, MayBeNone],
            "o?.pss",
        ),
    ];
    for (row, src, want, needle) in unchanged {
        let (_, ds) = checked(&src);
        let got: Vec<Code> = ds.iter().map(Diagnostic::code).collect();
        assert_eq!(got, want, "row `{row}`: {ds:?}");
        let err = ds
            .iter()
            .find(|d| d.code() == MayBeNone)
            .expect("row should have E0062");
        let line = src.lines().count() - 1;
        assert_eq!(at(&src, err.span()), on(&src, line, needle), "row `{row}`");
    }

    let rows = [
        (
            "2D struct field assign",
            format!("{decls}fn f(mut o: L<P>?) {{ o?.pss[0][0].x = 2 }}{main_p}"),
            "?.pss",
        ),
        (
            "2D struct mut self call",
            format!("{decls}fn f(mut o: L<P>?) {{ o?.pss[0][0].set() }}{main_p}"),
            "?.pss",
        ),
        (
            "3D struct mut self call",
            format!("{decls}fn f(mut o: L<P>?) {{ o?.psss[0][0][0].set() }}{main_p}"),
            "?.psss",
        ),
        (
            "2D enum mut self call",
            format!("{decls}fn f(mut o: L<P>?) {{ o?.dss[0][0].flip() }}{main_p}"),
            "?.dss",
        ),
        (
            "2D generic param mut self call",
            format!(
                "{decls}fn g<T: Rm>(mut o: L<T>?) {{ o?.tss[0][0].set() }}\nfn main() {{ let mut o: L<P>? = None; g(mut o); print(o != None) }}"
            ),
            "?.tss",
        ),
    ];
    for (row, src, needle) in rows {
        let (_, ds) = checked(&src);
        let got: Vec<Code> = ds.iter().map(Diagnostic::code).collect();
        assert_eq!(got, vec![MayBeNone], "row `{row}`: {ds:?}");
        let line = src.lines().count() - 1;
        assert_eq!(
            at(&src, ds[0].span()),
            on(&src, line, needle),
            "row `{row}`"
        );
    }
}

/// GIVEN an optional struct with a non-optional array field and an optional array field. WHEN a
/// function writes or calls `mut self` on an element after `?.`. THEN E0062 offers `replace` help
/// only when the indexed field is non-optional.
#[test]
fn indexed_safe_chain_should_keep_replace_help_only_for_present_array_fields() {
    judge_unused();
    let decls = "struct P { x: i64; fn set(mut self) { self.x = 1 } }\n\
                 struct L { ps: [P]; pss: [[P]]; qs: [[i64]]; ops: [P]?; oqs: [[i64]]? }\n";
    let main_call = "\nfn main() { let mut o: L? = None; f(mut o); print(o != None) }";
    let without_help = [
        (
            "field assign on optional array, unchanged",
            "o?.ops[0].x = 2",
        ),
        (
            "mut self call on optional array, unchanged",
            "o?.ops[0].set()",
        ),
        (
            "push on optional nested array, unchanged",
            "o?.oqs[0].push(2)",
        ),
    ];
    for (row, stmt) in without_help {
        let src = format!("{decls}fn f(mut o: L?) {{ {stmt} }}{main_call}");
        let d = only(&src, MayBeNone);
        assert!(d.help().is_none(), "row `{row}` should have no help: {d:?}");
    }

    let with_help = [
        ("field assign on present array", "o?.ps[0].x = 2"),
        ("mut self call on present array", "o?.ps[0].set()"),
        ("push on present nested array", "o?.qs[0].push(2)"),
        ("mut self call on present 2D array", "o?.pss[0][0].set()"),
    ];
    for (row, stmt) in with_help {
        let src = format!("{decls}fn f(mut o: L?) {{ {stmt} }}{main_call}");
        let d = only(&src, MayBeNone);
        assert!(d.help().is_some(), "row `{row}` should have help: {d:?}");
    }
}
