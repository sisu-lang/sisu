//! Structs in the checker: the declarations, construction, field reads, plain-`self` methods and
//! the place rules. Until structs lower, a program with a struct and no other error is E0083.

use super::*;
use Code::{
    ArgumentCount, ArgumentNeedsLabel, AssignToExpression, AssignToImmutable, AssignToSelf,
    BindingNotFound, CannotMatch, ConstantType, DefinedTwice, Expected, FieldOutOfOrder,
    FunctionNotFound, MayBeNone, Mismatch, MissingField, MutArgumentNotPlace, MutArgumentType,
    MutPlaceTwice, MutSelf, NeverReassigned, NoMember, NotAnInterface, ReservedWord,
    TooManyArguments, TypeAsValue, UnwrapNeedsOptional,
};

/// The word `record` is a name, the word `struct` is a keyword that no name can be, and the word
/// `move` is reserved.
#[test]
fn words_should_report_their_code() {
    let rows = [
        (
            "record",
            "fn main() {\n    let record = 1\n}".to_string(),
            vec![],
        ),
        (
            "struct",
            "fn main() {\n    let struct = 1\n}".to_string(),
            vec![(Expected, (2, 9))],
        ),
        (
            "move",
            "fn main() {\n    let move = 1\n}".to_string(),
            vec![(ReservedWord, (2, 9))],
        ),
    ];
    error_rows(&rows);
}

/// The codes of `src` until structs lower: `codes` and, when none of them is an error, the E0083
/// of the stopgap at the first `struct` keyword, in the order of the positions.
fn until_lowered(src: &str, mut codes: Vec<Wanted>) -> Vec<Wanted> {
    if codes.iter().any(|(c, _)| c.severity() == Severity::Error) {
        return codes;
    }
    let line = src
        .lines()
        .position(|l| l.starts_with("struct "))
        .expect("a row that waits for the lowering should declare a struct");
    codes.push((ReservedWord, (line + 1, 1)));
    codes.sort_by_key(|(_, at)| *at);
    codes
}

/// `codes_row` with the stopgap of `until_lowered`.
fn lowered_row(row: &str, src: &str, codes: Vec<Wanted>) {
    codes_row(row, src, &until_lowered(src, codes));
}

/// `error_rows` with the stopgap of `until_lowered` in each row.
fn lowered_error_rows(rows: Vec<ErrorRow<'_>>) {
    let rows: Vec<_> = rows
        .into_iter()
        .map(|(name, src, wanted)| {
            let wanted = until_lowered(&src, wanted);
            (name, src, wanted)
        })
        .collect();
    error_rows(&rows);
}

/// The one diagnostic of `src`, which is of `code` at `pos`; a failure names `row`.
fn only(row: &str, src: &str, code: Code, pos: (usize, usize)) -> Diagnostic {
    let (_, mut ds) = checked(src);
    assert_eq!(ds.len(), 1, "row `{row}`: {ds:?}");
    let d = ds.remove(0);
    assert_eq!((d.code(), at(src, d.span())), (code, pos), "row `{row}`");
    d
}

/// The one error of `src`, which is of `code` at `pos`; a failure names `row`.
fn only_error(row: &str, src: &str, code: Code, pos: (usize, usize)) -> Diagnostic {
    let (_, ds) = checked(src);
    let mut errors: Vec<_> = (ds.into_iter())
        .filter(|d| d.code().severity() == Severity::Error)
        .collect();
    assert_eq!(errors.len(), 1, "row `{row}`: {errors:?}");
    let d = errors.remove(0);
    assert_eq!((d.code(), at(src, d.span())), (code, pos), "row `{row}`");
    d
}

/// GIVEN `struct P { x: i64 }` and no other error THEN the error is E0083 at the keyword. A use
/// of the name adds no error. A program with another error has no E0083.
#[test]
fn struct_should_be_rejected_until_it_lowers() {
    let rejected = vec![(ReservedWord, (1, 1))];
    let rows = [
        (
            "a struct",
            "struct P { x: i64 }\nfn main() {}".to_string(),
            rejected.clone(),
        ),
        (
            "a mut field",
            "struct P { mut x: i64 }\nfn main() {}".to_string(),
            rejected.clone(),
        ),
        (
            "spaces after the keyword",
            "struct   P { x: i64 }\nfn main() {}".to_string(),
            rejected.clone(),
        ),
        (
            "a use of the name",
            "struct P { x: i64 }\nfn main() { let p: P = P(x: 1) }".to_string(),
            rejected,
        ),
        (
            "another error",
            "struct P { x: i64 }\nfn main() {\n    let p = P(x: 1)\n    p.x = 2\n}".to_string(),
            vec![(AssignToImmutable, (4, 5))],
        ),
    ];
    error_rows(&rows);
}

/// The two structs of the write rows: `In` and `T`, which holds an `In`.
const IN_T: &str = "struct In { n: i64; xs: [i64] }\nstruct T { i: In; n: i64 }\n";

/// GIVEN `struct In { n: i64; xs: [i64] }`, `struct T { i: In; n: i64 }`, and a root holding a
/// `T` that cannot change. WHEN the program writes through it. THEN the error is E0001 at the
/// write's root.
#[test]
fn write_through_a_root_that_cannot_change_should_be_e0001() {
    let local = |write: &str| {
        format!("{IN_T}fn main() {{\n    let t = T(i: In(n: 1, xs: [1]), n: 1)\n    {write}\n}}")
    };
    let rows = [
        ("a local, depth 1", local("t.n = 1"), (5, 5)),
        ("a local, depth 2", local("t.i.n = 1"), (5, 5)),
        ("a compound write", local("t.i.n += 1"), (5, 5)),
        ("an element", local("t.i.xs[0] = 1"), (5, 5)),
        ("a built-in mut call", local("t.i.xs.push(1)"), (5, 5)),
        (
            "a parameter",
            format!("{IN_T}fn f(t: T) {{\n    t.i.n = 1\n}}\nfn main() {{}}"),
            (4, 5),
        ),
        (
            "a receiver",
            "struct In { n: i64; xs: [i64] }\nstruct T {\n    i: In\n    n: i64\n    fn m(self) {\n        self.i.n = 1\n    }\n}\nfn main() {}"
                .to_string(),
            (6, 9),
        ),
        (
            "a for binding",
            format!("{IN_T}fn f(ts: [T]) {{\n    for t in ts {{\n        t.n = 1\n    }}\n}}\nfn main() {{}}"),
            (5, 9),
        ),
        (
            "an if let binding",
            format!("{IN_T}fn f(o: T?) {{\n    if let Some(t) = o {{\n        t.n = 1\n    }}\n}}\nfn main() {{}}"),
            (5, 9),
        ),
        (
            "a match binding",
            format!(
                "{IN_T}fn f(o: T?) {{\n    match o {{\n        Some(t) => {{\n            t.n = 1\n        }}\n        None => {{}}\n    }}\n}}\nfn main() {{}}"
            ),
            (6, 13),
        ),
    ];
    for (row, src, pos) in rows {
        codes_row(row, &src, &[(AssignToImmutable, pos)]);
    }
}

/// GIVEN a struct method with plain `self`. WHEN it writes a field, assigns to `self`, or another
/// method takes `mut self`. THEN the codes are E0001, E0073 and E0018, each with no fix and a
/// help.
#[test]
fn plain_self_should_report_its_codes() {
    let src = "struct P {\n    n: i64\n    fn a(self) {\n        self.n = 1\n    }\n    fn b(self) {\n        self = P(n: 2)\n    }\n    fn c(mut self) {}\n}\nfn main() {}";
    let (_, ds) = checked_with(PRELUDE_SOURCE, src);
    assert_eq!(
        errors(src, &ds),
        [
            (AssignToImmutable, (4, 9)),
            (AssignToSelf, (7, 9)),
            (MutSelf, (9, 10)),
        ]
    );
    for d in ds.iter().filter(|d| d.code().severity() == Severity::Error) {
        assert!(d.fixes().is_empty(), "{d:?}");
        assert!(d.help().is_some(), "{d:?}");
    }
}

/// GIVEN `struct P { n: i64; m: i64 }` WHEN a construction or a field read is wrong THEN the code
/// is the one that a class gives, at the same place.
#[test]
fn construction_should_report_the_class_codes() {
    let program =
        |body: &str| format!("struct P {{\n    n: i64\n    m: i64\n}}\nfn main() {{\n{body}\n}}");
    let rows = [
        (
            "a missing field",
            "    let p = P(n: 1)",
            (MissingField, (6, 13)),
        ),
        (
            "too many arguments",
            "    let p = P(n: 1, m: 2, 3)",
            (TooManyArguments, (6, 27)),
        ),
        (
            "an argument with no label",
            "    let p = P(1, m: 2)",
            (ArgumentNeedsLabel, (6, 15)),
        ),
        (
            "fields out of order",
            "    let p = P(m: 2, n: 1)",
            (FieldOutOfOrder, (6, 15)),
        ),
        (
            "an unknown field read",
            "    let p = P(n: 1, m: 2)\n    print(p.k)",
            (NoMember, (7, 13)),
        ),
    ];
    for (row, body, wanted) in rows {
        codes_row(row, &program(body), &[wanted]);
    }
    // A field read from the classes with the struct's id would find `s: string`.
    let src = "class C { s: string }\nstruct P { n: i64 }\nfn main() {\n    let p = P(n: \"x\")\n}";
    assert_eq!(found(src), [(Mismatch, (4, 18), vec![(2, 15)])]);
}

/// GIVEN a struct name. WHEN it is declared twice, used where it cannot be, or misspelled. THEN
/// the code and its place are those of a class name.
#[test]
fn struct_name_should_resolve_as_a_class_name_does() {
    let s = "struct P { x: i64 }";
    let twice = |first: &str, second: &str| format!("{first}\n{second}\nfn main() {{}}");
    let mut rows = vec![
        ("a struct twice", twice(s, s), vec![(DefinedTwice, (2, 8))]),
        (
            "a broken struct and its uses",
            "struct P {\n    x i64\n}\nfn main() {\n    let p: P = P(x: 1)\n}".to_string(),
            vec![(Expected, (2, 7))],
        ),
        (
            "a value",
            format!("{s}\nfn main() {{\n    let x = P\n}}"),
            vec![(TypeAsValue, (3, 13))],
        ),
        (
            "a bound",
            format!("{s}\nfn f<T: P>() {{}}\nfn main() {{}}"),
            vec![(NotAnInterface, (2, 9))],
        ),
        (
            "a pattern",
            "struct P {\n    x: i64\n}\nfn main() {\n    let o: i64? = None\n    match o {\n        P(x) => {}\n        _ => {}\n    }\n}".to_string(),
            vec![(Mismatch, (7, 9))],
        ),
    ];
    let others = [
        ("a class", "class P { x: i64 }", 7),
        ("an enum", "enum P { A }", 6),
        ("a function", "fn P() {}", 4),
        ("an interface", "interface P {}", 11),
        ("a constant", "const P: i64 = 1", 7),
        ("an alias", "type P = i64", 6),
    ];
    for (name, other, col) in others {
        rows.push((name, twice(other, s), vec![(DefinedTwice, (2, 8))]));
        rows.push((name, twice(s, other), vec![(DefinedTwice, (2, col))]));
    }
    error_rows(&rows);
    let src = "struct Point { x: i64 }\nfn main() {\n    let p = Pointt(x: 1)\n}";
    let d = diagnostic(src, FunctionNotFound, (3, 13, "Pointt"), &[]);
    let edits: Vec<_> = fixes(src, &d).into_iter().flat_map(|(_, e)| e).collect();
    assert_eq!(edits, [(3, 13, "Pointt", "Point".to_string())]);
}

/// GIVEN `let p = P(x: 1)` of a struct `P` WHEN a pattern names `P` THEN `match` is E0124 and
/// `if let` is E0051, as for a class.
#[test]
fn pattern_on_a_struct_should_be_e0124() {
    let src = "struct P {\n    x: i64\n}\nfn main() {\n    let p = P(x: 1)\n    match p {\n        P(x) => {}\n    }\n    if let P(x) = p {}\n}";
    let (_, ds) = checked_with(PRELUDE_SOURCE, src);
    assert_eq!(
        errors(src, &ds),
        [(CannotMatch, (6, 11)), (UnwrapNeedsOptional, (9, 19))]
    );
}

/// The declarations of the `?.` write rows: a class `C` and an enum `E`, each with its own
/// `push`, and structs that hold them.
const CHAIN: &str = "class C {
    mut n: i64
    mut p: P
    fn push(self, x: i64) {
        print(x)
    }
}
enum E {
    A
    fn push(self, x: i64) {
        print(x)
    }
}
struct P { n: i64 }
struct J { n: i64 }
struct I { n: i64; j: J; c: C; xs: [i64] }
struct In { n: i64; xs: [i64]; ys: [P]; i: I; c: C; e: E }
struct S { o: In? }
fn add(mut a: i64, b: i64) {
    a += b
}
fn f() -> P? {
    None
}
fn h() -> In? {
    None
}
";

/// A program of `CHAIN` whose `main` runs `setup`, then `write` on the line that `CHAIN_LINE`
/// names.
fn chain(setup: &str, write: &str) -> String {
    format!("{CHAIN}fn main() {{\n    {setup}\n    {write}\n}}")
}

/// The line of `write` in `chain`.
const CHAIN_LINE: usize = 30;

/// GIVEN an optional struct. WHEN a write goes through a `?.` on it. THEN a root that cannot
/// change is E0001. Any other root is one E0062 at that `?.`, with no fix.
#[test]
fn chain_write_on_a_struct_should_report_by_its_root() {
    let l = CHAIN_LINE;
    let o = "let mut o: In? = None";
    // Exactly one diagnostic, E0062 at the first `?.`, with no fix and no W0002.
    for write in [
        "o?.n = 1",
        "o?.n += 1",
        "o?.xs[0] = 1",
        "o?.i.n = 1",
        "o?.xs.push(1)",
        "o?.xs?.push(1)",
        "o?.ys[0].n = 1",
        "o?.i.j.n = 1",
    ] {
        let d = only(write, &chain(o, write), MayBeNone, (l, 6));
        assert!(d.fixes().is_empty(), "row `{write}`: {d:?}");
    }
    let s = only(
        "s.o?.n",
        &chain("let mut s = S(o: None)", "s.o?.n = 1"),
        MayBeNone,
        (l, 8),
    );
    assert!(s.fixes().is_empty(), "{s:?}");
    // The form that a help would name is refused too, so there is no help.
    let unhelped = [
        ("f()?.n", chain("let x = 1", "f()?.n = x"), (l, 8)),
        ("h()?.i.n", chain("let x = 1", "h()?.i.n = x"), (l, 8)),
        ("c?.p.n", chain("let c: C? = None", "c?.p.n = 1"), (l, 6)),
        ("c?.p?.n", chain("let c: C? = None", "c?.p?.n = 1"), (l, 9)),
        (
            "s?.o?.n",
            chain("let mut s: S? = None", "s?.o?.n = 1"),
            (l, 9),
        ),
        ("o?.i.c.n", chain(o, "o?.i.c.n = 1"), (l, 6)),
    ];
    for (row, src, pos) in unhelped {
        let d = only(row, &src, MayBeNone, pos);
        assert!(d.fixes().is_empty(), "row `{row}`: {d:?}");
        assert!(d.help().is_none(), "row `{row}`: {d:?}");
    }
    // A root that cannot change.
    let rooted = [
        ("let o", chain("let o: In? = None", "o?.n = 1")),
        ("let o, depth 2", chain("let o: In? = None", "o?.i.n = 1")),
        (
            "let o, an element",
            chain("let o: In? = None", "o?.ys[0].n = 1"),
        ),
        (
            "a parameter",
            format!("{CHAIN}fn g(o: In?) {{\n    o?.n = 1\n}}\nfn main() {{}}"),
        ),
        (
            "a parameter, depth 2",
            format!("{CHAIN}fn g(o: In?) {{\n    o?.i.n = 1\n}}\nfn main() {{}}"),
        ),
    ];
    for (row, src) in rooted {
        let line = src
            .lines()
            .position(|x| x.starts_with("    o?."))
            .expect("a write")
            + 1;
        only(row, &src, AssignToImmutable, (line, 5));
    }
    // A `mut` argument and a `push` of no array write nothing through the `?.`.
    let d = only_error(
        "add(mut o?.n)",
        &chain(o, "add(mut o?.n, 1)"),
        MutArgumentType,
        (l, 13),
    );
    assert!(d.help().is_none(), "{d:?}");
    let d = only_error(
        "add(mut o?.i.n)",
        &chain(o, "add(mut o?.i.n, 1)"),
        MayBeNone,
        (l, 13),
    );
    assert!(d.fixes().is_empty(), "{d:?}");
    assert!(d.help().is_none(), "{d:?}");
    only_error(
        "o?.i.c.push",
        &chain(o, "o?.i.c.push(1)"),
        MayBeNone,
        (l, 5),
    );
    only_error("o?.c.push", &chain(o, "o?.c.push(1)"), MayBeNone, (l, 5));
    // A call or a write past a class object writes nothing in the struct.
    lowered_error_rows(vec![
        ("o?.c?.push", chain(o, "o?.c?.push(1)"), vec![]),
        ("o?.e?.push", chain(o, "o?.e?.push(1)"), vec![]),
        ("o?.c?.n", chain(o, "o?.c?.n = 1"), vec![]),
    ]);
    // A read of the same path keeps its help.
    let d = only(
        "print",
        &chain("let o: In? = None", "print(o?.ys[0].n)"),
        MayBeNone,
        (l, 11),
    );
    assert!(d.help().is_some(), "{d:?}");
}

/// GIVEN a struct `P`, `fn f() -> P` and `add(mut a: i64, b: i64)`. WHEN a write stays inside the
/// value that a call or a construction gives. THEN it is E0021, and a `mut` argument is E0169. A
/// path that crosses a class object, or a `push` of a field's own type, has no error.
#[test]
fn write_through_a_temporary_should_be_e0021() {
    let program = |body: &str| {
        format!(
            "class C {{
    mut n: i64
    mut i: In
    fn push(self, x: i64) {{
        print(x)
    }}
}}
enum E {{
    A
    fn push(self, x: i64) {{
        print(x)
    }}
}}
struct In {{ n: i64; xs: [i64] }}
struct P {{ n: i64; i: In; xs: [i64]; c: C; e: E; cs: [C] }}
struct Q {{ n: i64 }}
fn f() -> P {{
    P(n: 1, i: In(n: 1, xs: []), xs: [], c: C(n: 1, i: In(n: 1, xs: [])), e: E.A, cs: [])
}}
fn add(mut a: i64, b: i64) {{
    a += b
}}
fn main() {{
    {body}
}}"
        )
    };
    let l = 24;
    let rows = [
        ("f().n", "f().n = 2", vec![(AssignToExpression, (l, 5))]),
        (
            "Q(n: 1).n",
            "Q(n: 1).n = 2",
            vec![(AssignToExpression, (l, 5))],
        ),
        ("f().i.n", "f().i.n = 2", vec![(AssignToExpression, (l, 5))]),
        (
            "f().xs[0]",
            "f().xs[0] = 1",
            vec![(AssignToExpression, (l, 5))],
        ),
        (
            "f().xs.push",
            "f().xs.push(1)",
            vec![(AssignToExpression, (l, 5))],
        ),
        (
            "add(mut f().n)",
            "add(mut f().n, 1)",
            vec![(MutArgumentNotPlace, (l, 13))],
        ),
        (
            "add(mut f().xs[0])",
            "add(mut f().xs[0], 1)",
            vec![(MutArgumentNotPlace, (l, 13))],
        ),
        ("f().c.n", "f().c.n = 2", vec![]),
        ("f().c.i.n", "f().c.i.n = 2", vec![]),
        ("f().c.push", "f().c.push(1)", vec![]),
        ("f().e.push", "f().e.push(1)", vec![]),
        ("f().cs[0].push", "f().cs[0].push(1)", vec![]),
    ];
    for (row, body, wanted) in rows {
        lowered_row(row, &program(body), wanted);
    }
}

/// GIVEN `let mut o: In? = None` of a struct `In`. WHEN a `.` write goes through `o`. THEN it is
/// one E0062 at `o`, with no fix, a help, and no W0002. On a class, the fix and the help stay.
#[test]
fn dot_write_on_an_optional_struct_should_offer_no_false_fix() {
    let l = CHAIN_LINE;
    let o = "let mut o: In? = None";
    for write in [
        "o.n = 1",
        "o.xs.push(1)",
        "o.xs[0] = 1",
        "o.i.n = 1",
        "o.ys[0].n = 1",
        "o.i.xs.push(1)",
    ] {
        let d = only(write, &chain(o, write), MayBeNone, (l, 5));
        assert!(d.fixes().is_empty(), "row `{write}`: {d:?}");
        assert!(d.help().is_some(), "row `{write}`: {d:?}");
    }
    let d = only(
        "h().i.n",
        &chain("let x = 1", "h().i.n = x"),
        MayBeNone,
        (l, 5),
    );
    assert!(d.fixes().is_empty(), "{d:?}");
    assert!(d.help().is_none(), "{d:?}");
    let d = only("o.i.c.n", &chain(o, "o.i.c.n = 1"), MayBeNone, (l, 5));
    assert!(d.fixes().is_empty(), "{d:?}");
    let class = |write: &str| {
        format!(
            "class K {{\n    mut n: i64\n    mut xs: [i64]\n}}\nfn main() {{\n    let mut k: K? = None\n    {write}\n}}"
        )
    };
    for (write, fixed) in [
        ("k.n = 1", true),
        ("k.xs.push(1)", true),
        ("k.xs[0] = 1", false),
    ] {
        let d = only_error(write, &class(write), MayBeNone, (7, 5));
        assert_eq!(!d.fixes().is_empty(), fixed, "row `{write}`: {d:?}");
        assert!(d.help().is_some(), "row `{write}`: {d:?}");
    }
}

/// GIVEN an index that can exit WHEN it is a step of a path THEN the path is a place. A block that
/// the user wrote as the root is a temporary.
#[test]
fn index_that_exits_should_keep_the_path() {
    let program = |body: &str| {
        format!(
            "class K {{
    mut n: i64
    fn push(self, x: i64) {{
        print(x)
    }}
}}
class C {{
    mut xs: [P]
}}
struct P {{ n: i64; c: K }}
struct T {{ xs: [P] }}
fn g(o: i64?, t0: T, c: C, xs: [P]) {{
    {body}
}}
fn main() {{}}"
        )
    };
    lowered_error_rows(vec![
        (
            "t.xs[o ?? return].n",
            program("let mut t = t0\n    t.xs[o ?? return].n = 1"),
            vec![],
        ),
        (
            "c.xs[o ?? return].n",
            program("c.xs[o ?? return].n = 1"),
            vec![],
        ),
        (
            "t.xs[o ?? return].c.push",
            program("let t = t0\n    t.xs[o ?? return].c.push(1)"),
            vec![],
        ),
        (
            "a block",
            program("({ let a = xs; a[0] }).n = 1"),
            vec![(AssignToExpression, (13, 5))],
        ),
    ]);
}

/// GIVEN a struct `P` in a `mut` field of a class `C`. WHEN a write goes through that field. THEN
/// it has no error. Through a field with no `mut`, it is E0001.
#[test]
fn struct_under_a_class_should_take_the_field_rule() {
    let program = |body: &str| {
        format!(
            "class C {{
    mut p: P
    mut xs: [P]
    fn m(self) {{
        self.p.n = 2
    }}
}}
class D {{
    p: P
}}
struct P {{ n: i64; xs: [i64] }}
struct S {{ c: C }}
fn f() -> S {{
    S(c: C(p: P(n: 1, xs: []), xs: []))
}}
fn put(mut a: [i64], b: i64) {{
    a.push(b)
}}
fn g(c0: C, cs: [C], d: D) {{
    let c = c0
    {body}
}}
fn main() {{}}"
        )
    };
    let mut rows = Vec::new();
    for write in [
        "c.p.n = 2",
        "c.xs[0].n = 1",
        "c.p.xs.push(1)",
        "c.p.xs[0] = 1",
        "put(mut c.p.xs, 1)",
        "cs[0].p.n = 2",
        "f().c.p.n = 2",
    ] {
        rows.push((write, program(write), vec![]));
        if !write.starts_with("f()") {
            let param = write.replacen("c.", "c0.", 1);
            rows.push((write, program(&param), vec![]));
        }
    }
    rows.push((
        "d.p.n",
        program("d.p.n = 2"),
        vec![(AssignToImmutable, (21, 5))],
    ));
    lowered_error_rows(rows);
}

/// GIVEN programs with no struct THEN each has the codes it has today. With a struct, a write
/// through a class field has no error.
#[test]
fn class_program_should_report_as_today() {
    let program = |body: &str| {
        format!(
            "class C {{\n    mut n: i64\n}}\nenum E {{\n    A\n}}\nconst K1: i64 = 1\nfn f() -> C {{\n    C(n: 1)\n}}\nfn add(a: i64) {{}}\nfn main() {{\n    {body}\n}}"
        )
    };
    let rows = [
        ("let c", "let c = C(n: 1)\n    c.n = 1", vec![]),
        ("let c = f()", "let c = f()\n    c.n = 1", vec![]),
        (
            "let mut c",
            "let mut c = f()\n    c.n = 1",
            vec![(NeverReassigned, (13, 13))],
        ),
        (
            "let mut cs",
            "let mut cs = [f()]\n    cs[0].n = 1",
            vec![(NeverReassigned, (13, 13))],
        ),
        ("add.n", "add.n = 1", vec![(TypeAsValue, (13, 5))]),
        ("C.n", "C.n = 1", vec![(TypeAsValue, (13, 5))]),
        ("K1.n", "K1.n = 1", vec![(NoMember, (13, 8))]),
        ("E.A.n", "E.A.n = 1", vec![(Code::EnumField, (13, 9))]),
        ("nope.n", "nope.n = 1", vec![(BindingNotFound, (13, 5))]),
    ];
    for (row, body, wanted) in rows {
        codes_row(row, &program(body), &wanted);
    }
    let with_struct = |body: &str| format!("{}\nstruct S {{ c: C }}", program(body));
    lowered_row(
        "let s",
        &with_struct("let s = S(c: C(n: 1))\n    s.c.n = 1"),
        vec![],
    );
    lowered_row(
        "let mut s",
        &with_struct("let mut s = S(c: C(n: 1))\n    s.c.n = 1"),
        vec![(NeverReassigned, (13, 13))],
    );
}

/// GIVEN a `let mut` struct WHEN a write changes it THEN it is not W0002, and a `push` of a
/// class's own is.
#[test]
fn struct_write_should_keep_its_marks() {
    let decls = "class C {\n    mut n: i64\n    fn push(self, x: i64) {\n        print(x)\n    }\n}\nstruct P { n: i64 }\nstruct T { xs: [i64] }\nstruct S { c: C }\n";
    let program = |body: &str| format!("{decls}fn main() {{\n    {body}\n}}");
    lowered_row("t.n", &program("let mut t = P(n: 1)\n    t.n = 1"), vec![]);
    lowered_row(
        "t.xs.push",
        &program("let mut t = T(xs: [])\n    t.xs.push(1)"),
        vec![],
    );
    lowered_row(
        "t.c.push",
        &program("let mut t = S(c: C(n: 1))\n    t.c.push(1)"),
        vec![(NeverReassigned, (11, 13))],
    );
}

/// GIVEN a `let`, a plain parameter and a pattern binding of a struct `T`. WHEN `t.n = 2` and
/// `t.i.xs[0] = 1` write through it. THEN each is E0001 at the write.
#[test]
fn struct_part_should_report_its_own_text() {
    let src = format!(
        "{IN_T}fn f(t: T) {{\n    t.n = 2\n    t.i.xs[0] = 1\n}}\nfn g(o: T?) {{\n    if let Some(t) = o {{\n        t.n = 2\n        t.i.xs[0] = 1\n    }}\n}}\nfn main() {{\n    let t = T(i: In(n: 1, xs: [1]), n: 1)\n    t.n = 2\n    t.i.xs[0] = 1\n}}"
    );
    codes_row(
        "parts",
        &src,
        &[
            (AssignToImmutable, (4, 5)),
            (AssignToImmutable, (5, 5)),
            (AssignToImmutable, (9, 9)),
            (AssignToImmutable, (10, 9)),
            (AssignToImmutable, (15, 5)),
            (AssignToImmutable, (16, 5)),
        ],
    );
}

/// GIVEN a struct path WHEN it is a `mut` argument THEN a root that cannot change is E0169, and
/// the same place twice is E0171.
#[test]
fn struct_mut_argument_should_report_by_its_root_and_overlap() {
    let decls = "struct In { n: i64; xs: [i64] }\nstruct T {\n    i: In\n    n: i64\n    a: i64\n    fn m(self) {\n        bump(mut self.n, 1)\n    }\n}\nfn add(mut a: [i64], b: i64) {\n    a.push(b)\n}\nfn bump(mut a: i64, b: i64) {\n    a += b\n}\nfn two(mut a: i64, mut b: i64) {\n    a = b\n    b = a\n}\n";
    let program = |body: &str| format!("{decls}fn main() {{\n    {body}\n}}");
    let src = program("let t = T(i: In(n: 1, xs: []), n: 1, a: 1)\n    add(mut t.i.xs, 9)");
    codes_row(
        "let t",
        &src,
        &[
            (MutArgumentNotPlace, (7, 18)),
            (MutArgumentNotPlace, (22, 13)),
        ],
    );
    let src = program("let mut t = T(i: In(n: 1, xs: []), n: 1, a: 1)\n    two(mut t.a, mut t.a)");
    codes_row(
        "twice",
        &src,
        &[(MutArgumentNotPlace, (7, 18)), (MutPlaceTwice, (22, 22))],
    );
}

/// GIVEN a struct method with plain `self`. WHEN it changes a part of `self`. THEN it is E0001. A
/// write past a class object has no error.
#[test]
fn plain_self_write_should_report_by_its_form() {
    let program = |body: &str| {
        format!(
            "class C {{\n    mut n: i64\n}}\nstruct P {{\n    n: i64\n    xs: [i64]\n    c: C\n    fn m(self) {{\n        {body}\n    }}\n}}\nfn main() {{}}"
        )
    };
    lowered_row(
        "push",
        &program("self.xs.push(1)"),
        vec![(AssignToImmutable, (9, 9))],
    );
    lowered_row(
        "+=",
        &program("self.n += 1"),
        vec![(AssignToImmutable, (9, 9))],
    );
    lowered_row("a class", &program("self.c.n = 2"), vec![]);
}

/// GIVEN a base that fails WHEN a struct path writes through it THEN the base's error is the only
/// one.
#[test]
fn broken_base_should_report_once() {
    let decls = "class Cn {\n    mut n: i64\n}\nclass A {\n    c: Cn\n}\nstruct P { n: i64 }\nstruct T { xs: [P]; n: i64 }\nfn f() -> P {\n    P(n: 1)\n}\n";
    let program = |body: &str| format!("{decls}fn main() {{\n    {body}\n}}");
    let l = 14;
    lowered_row(
        "t.nope",
        &program("let t = T(xs: [], n: 1)\n    t.nope.n = 1"),
        vec![(NoMember, (l, 7))],
    );
    lowered_row(
        "t.xs[nope]",
        &program("let mut t = T(xs: [], n: 1)\n    t.xs[nope].n = 1"),
        vec![(BindingNotFound, (l, 10))],
    );
    lowered_row(
        "t.xs[nope] +=",
        &program("let mut t = T(xs: [], n: 1)\n    t.xs[nope].n += 1"),
        vec![(BindingNotFound, (l, 10))],
    );
    lowered_row(
        "f(\"x\")",
        &program("f(\"x\").n = 2"),
        vec![(ArgumentCount, (13, 5))],
    );
    let src = format!("{decls}const K: P = P(n: 1)\nfn main() {{\n    K.n = 1\n}}");
    lowered_row("K.n", &src, vec![(ConstantType, (12, 10))]);
    lowered_row(
        "a.c.n",
        &program("let a = A(c: Cn(n: 1))\n    a.c.n = 1"),
        vec![],
    );
}

/// GIVEN a path of struct fields, indexes, a class's method and a compound write THEN each walks
/// its steps and reports only the stopgap.
#[test]
fn struct_path_should_walk_each_step() {
    let decls = "class C {\n    mut n: i64\n    fn push(self, x: i64) {\n        print(x)\n    }\n}\nstruct S { c: C }\nstruct V { ys: [i64] }\nstruct W { i: V }\nstruct U { s: string }\n";
    let program = |body: &str| format!("{decls}fn main() {{\n    {body}\n}}");
    lowered_row(
        "s.c.push",
        &program("let s = S(c: C(n: 1))\n    s.c.push(1)"),
        vec![],
    );
    lowered_row(
        "xs[0].i.ys[1]",
        &program("let mut xs = [W(i: V(ys: [1, 2]))]\n    xs[0].i.ys[1] = 2"),
        vec![],
    );
    lowered_row(
        "t.s +=",
        &program("let mut t = U(s: \"\")\n    t.s += \"x\""),
        vec![],
    );
}

/// GIVEN a generic struct, a conformance list, a generic method in a struct's body or an `extend`
/// of a struct. THEN the only error is E0083 at the `struct` keyword.
#[test]
fn generic_struct_should_be_rejected_until_task_6() {
    let rows = [
        (
            "a generic struct",
            "struct P<T> { x: T }\nfn main() {\n    let p = P(x: 1)\n    print(p.x)\n}",
            (1, 1),
        ),
        (
            "a conformance list",
            "interface K {}\nstruct P: K { x: i64 }\nfn main() {}",
            (2, 1),
        ),
        (
            "a generic method",
            "struct P {\n    x: i64\n    fn m<T>(self, t: T) {}\n}\nfn main() {}",
            (1, 1),
        ),
        (
            "an extend",
            "struct P { x: i64 }\nextend P {}\nfn main() {}",
            (1, 1),
        ),
    ];
    for (row, src, pos) in rows {
        codes_row(row, src, &[(ReservedWord, pos)]);
    }
}

/// GIVEN a write through a path of `.xs[0]` steps in a struct `T`. WHEN the path is 10, 20 and 30
/// steps deep. THEN the path walks grow in step with the depth, so each step is walked once.
#[test]
fn struct_path_should_check_each_step_once() {
    let steps = |depth: usize| {
        let path = ".xs[0]".repeat(depth);
        let src = format!(
            "struct T {{ xs: [T]; n: i64 }}\nfn main() {{\n    let mut t = T(xs: [], n: 1)\n    t{path}.n = 2\n    print(t.n)\n}}"
        );
        PATH_STEPS.set(0);
        let (_, ds) = checked(&src);
        assert!(ds.iter().all(|d| d.code() == ReservedWord), "{ds:?}");
        PATH_STEPS.get()
    };
    assert_near_linear(steps(10), steps(20), steps(30));
}
