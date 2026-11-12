//! Structs in the checker: the declarations, construction, field reads, plain-`self` methods and
//! the place rules.

use super::*;
use Code::{
    ArgumentCount, ArgumentNeedsLabel, AssignToExpression, AssignToImmutable, AssignToSelf,
    BindingNotFound, CannotExtend, CannotInfer, CannotMatch, ConstantType, DefinedTwice, Expected,
    FieldOutOfOrder, FunctionNotFound, InstantiationCycle, IsNeedsObjects, MayBeNone,
    MethodAsFunction, Mismatch, MissingField, MutArgumentNotPlace, MutArgumentType, MutPlaceTwice,
    MutSelf, NeverConstructed, NeverReassigned, NoMember, NoOperator, NotAnInterface, ReservedWord,
    TooManyArguments, TypeAsValue, UnusedVariable, UnwrapNeedsOptional,
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
    error_rows(&[
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
        codes_row(row, &program(body), &wanted);
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
    error_rows(&[
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
    error_rows(&rows);
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
    codes_row(
        "let s",
        &with_struct("let s = S(c: C(n: 1))\n    s.c.n = 1"),
        &[],
    );
    codes_row(
        "let mut s",
        &with_struct("let mut s = S(c: C(n: 1))\n    s.c.n = 1"),
        &[(NeverReassigned, (13, 13))],
    );
}

/// GIVEN a `let mut` struct WHEN a write changes it THEN it is not W0002, and a `push` of a
/// class's own is.
#[test]
fn struct_write_should_keep_its_marks() {
    let decls = "class C {\n    mut n: i64\n    fn push(self, x: i64) {\n        print(x)\n    }\n}\nstruct P { n: i64 }\nstruct T { xs: [i64] }\nstruct S { c: C }\n";
    let program = |body: &str| format!("{decls}fn main() {{\n    {body}\n}}");
    codes_row("t.n", &program("let mut t = P(n: 1)\n    t.n = 1"), &[]);
    codes_row(
        "t.xs.push",
        &program("let mut t = T(xs: [])\n    t.xs.push(1)"),
        &[],
    );
    codes_row(
        "t.c.push",
        &program("let mut t = S(c: C(n: 1))\n    t.c.push(1)"),
        &[(NeverReassigned, (11, 13))],
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
    codes_row(
        "push",
        &program("self.xs.push(1)"),
        &[(AssignToImmutable, (9, 9))],
    );
    codes_row(
        "+=",
        &program("self.n += 1"),
        &[(AssignToImmutable, (9, 9))],
    );
    codes_row("a class", &program("self.c.n = 2"), &[]);
}

/// GIVEN a base that fails WHEN a struct path writes through it THEN the base's error is the only
/// one.
#[test]
fn broken_base_should_report_once() {
    let decls = "class Cn {\n    mut n: i64\n}\nclass A {\n    c: Cn\n}\nstruct P { n: i64 }\nstruct T { xs: [P]; n: i64 }\nfn f() -> P {\n    P(n: 1)\n}\n";
    let program = |body: &str| format!("{decls}fn main() {{\n    {body}\n}}");
    let l = 14;
    codes_row(
        "t.nope",
        &program("let t = T(xs: [], n: 1)\n    t.nope.n = 1"),
        &[(NoMember, (l, 7))],
    );
    codes_row(
        "t.xs[nope]",
        &program("let mut t = T(xs: [], n: 1)\n    t.xs[nope].n = 1"),
        &[(BindingNotFound, (l, 10))],
    );
    codes_row(
        "t.xs[nope] +=",
        &program("let mut t = T(xs: [], n: 1)\n    t.xs[nope].n += 1"),
        &[(BindingNotFound, (l, 10))],
    );
    codes_row(
        "f(\"x\")",
        &program("f(\"x\").n = 2"),
        &[(ArgumentCount, (13, 5))],
    );
    let src = format!("{decls}const K: P = P(n: 1)\nfn main() {{\n    K.n = 1\n}}");
    codes_row("K.n", &src, &[(ConstantType, (12, 10))]);
    codes_row(
        "a.c.n",
        &program("let a = A(c: Cn(n: 1))\n    a.c.n = 1"),
        &[],
    );
}

/// GIVEN a path of struct fields, indexes, a class's method and a compound write THEN each walks
/// its steps and reports only the stopgap.
#[test]
fn struct_path_should_walk_each_step() {
    let decls = "class C {\n    mut n: i64\n    fn push(self, x: i64) {\n        print(x)\n    }\n}\nstruct S { c: C }\nstruct V { ys: [i64] }\nstruct W { i: V }\nstruct U { s: string }\n";
    let program = |body: &str| format!("{decls}fn main() {{\n    {body}\n}}");
    codes_row(
        "s.c.push",
        &program("let s = S(c: C(n: 1))\n    s.c.push(1)"),
        &[],
    );
    codes_row(
        "xs[0].i.ys[1]",
        &program("let mut xs = [W(i: V(ys: [1, 2]))]\n    xs[0].i.ys[1] = 2"),
        &[],
    );
    codes_row(
        "t.s +=",
        &program("let mut t = U(s: \"\")\n    t.s += \"x\""),
        &[],
    );
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

/// GIVEN a struct `P` with the methods `get` and `twice`. WHEN a call names a method on a value,
/// on `self` or through `?.`. THEN the only error is the stopgap. A bare name of the method is
/// E0070, as it is for a class.
#[test]
fn struct_method_call_should_check_as_a_class_method_does() {
    let decls = "struct P {\n    n: i64\n    fn get(self) -> i64 {\n        self.n\n    }\n    fn twice(self) -> i64 {\n        self.get() + self.get()\n    }\n}\n";
    let program = |body: &str| format!("{decls}fn main() {{\n    {body}\n}}");
    let rows = [
        ("p.get()", program("let p = P(n: 1)\n    print(p.get())"), vec![]),
        ("self.get()", program(""), vec![]),
        (
            "o?.get()",
            program("let o: P? = P(n: 1)\n    print(o?.get() ?? 0)"),
            vec![],
        ),
        (
            "get(p)",
            program("let p = P(n: 1)\n    print(get(p))"),
            vec![(MethodAsFunction, (12, 11))],
        ),
        (
            "get() in a method",
            "struct P {\n    n: i64\n    fn get(self) -> i64 {\n        self.n\n    }\n    fn twice(self) -> i64 {\n        get()\n    }\n}\nfn main() {}".to_string(),
            vec![(MethodAsFunction, (7, 9))],
        ),
    ];
    error_rows(&rows);
}

/// GIVEN a struct method that is never called. WHEN it calls `push` on a field of `self`. THEN
/// each gives the code of the class form: none for a class's own `push`, E0063 for a type with
/// no `push`, and E0001 for an array, which a plain `self` cannot change.
#[test]
fn struct_self_push_should_report_as_the_class_form() {
    let program = |body: &str| {
        format!(
            "class C {{\n    fn push(self, x: i64) {{\n        print(x)\n    }}\n}}\nstruct In {{ xs: [[i64]] }}\nstruct P {{\n    c: C\n    n: i64\n    s: string\n    i: In\n    fn m(self) {{\n        {body}\n    }}\n}}\nfn main() {{}}"
        )
    };
    let rows = [
        ("self.c.push", program("self.c.push(1)"), vec![]),
        (
            "self.n.push",
            program("self.n.push(1)"),
            vec![(NoMember, (13, 16))],
        ),
        (
            "self.s.push",
            program("self.s.push(1)"),
            vec![(NoMember, (13, 16))],
        ),
        (
            "self.i.xs[0].push",
            program("self.i.xs[0].push(1)"),
            vec![(AssignToImmutable, (13, 9))],
        ),
    ];
    error_rows(&rows);
}

/// GIVEN a path of `.xs[0]`, `.os[0]` or `.os[0]?` steps in a struct `T`, 10, 20 and 30 steps
/// deep. WHEN the path ends in a class's own `push`, or a `?.` write starts it. THEN the path
/// walks grow in step with the depth.
#[test]
fn struct_push_and_chain_path_should_check_each_step_once() {
    let rows: [(&str, &str, &str, &str); 4] = [
        (
            "a class's own push",
            "let t = T(xs: [], os: [], c: C())\n    t",
            ".xs[0]",
            ".c.push(1)",
        ),
        (
            "a `?.` write",
            "let mut o: T? = None\n    o?",
            ".xs[0]",
            ".n = 1",
        ),
        (
            "a `?.` at each step",
            "let mut o: T? = None\n    o?",
            ".os[0]?",
            ".n = 1",
        ),
        (
            "`.` steps after an optional element",
            "let mut o: T? = None\n    o?.os[0]",
            ".os[0]",
            ".n = 1",
        ),
    ];
    for (row, root, step, end) in rows {
        let steps = |depth: usize| {
            let path = step.repeat(depth);
            let src = format!(
                "class C {{\n    fn push(self, x: i64) {{\n        print(x)\n    }}\n}}\nstruct T {{ xs: [T]; os: [T?]; c: C; n: i64 }}\nfn main() {{\n    {root}{path}{end}\n}}"
            );
            PATH_STEPS.set(0);
            let (_, ds) = checked(&src);
            assert!(
                ds.iter()
                    .filter(|d| d.code().severity() == Severity::Error)
                    .count()
                    == 1,
                "row `{row}`: {ds:?}"
            );
            PATH_STEPS.get()
        };
        assert_near_linear_for(row, steps(10), steps(20), steps(30));
    }
}

/// GIVEN `struct P { n: i64 }` and `type A = P`. WHEN the program constructs `A`. THEN it
/// constructs `P`, with the codes of a class alias.
#[test]
fn struct_alias_should_construct_its_struct() {
    let program = |aliases: &str, body: &str| {
        format!("struct P {{ n: i64 }}\n{aliases}\nfn main() {{\n    {body}\n}}")
    };
    let rows = [
        (
            "A(n: 1)",
            program("type A = P", "let a = A(n: 1)\n    print(a.n)"),
            vec![],
        ),
        (
            "an alias of an alias",
            program(
                "type A = P\ntype B = A",
                "let a: A = B(n: 1)\n    print(a.n)",
            ),
            vec![],
        ),
        (
            "A()",
            program("type A = P", "let a = A()"),
            vec![(MissingField, (4, 13))],
        ),
        (
            "A(n: true)",
            program("type A = P", "let a = A(n: true)"),
            vec![(Mismatch, (4, 18))],
        ),
        (
            "a generic alias",
            program("type A<T> = P", "let a = A(n: 1)"),
            vec![(CannotInfer, (4, 13))],
        ),
    ];
    error_rows(&rows);
}

/// GIVEN a struct `P`. WHEN `<` compares two of its values. THEN it is E0159, as for a class.
#[test]
fn struct_order_should_be_e0159() {
    let src = "struct P { n: i64 }\nfn main() {\n    print(P(n: 1) < P(n: 2))\n}";
    codes_row("P < P", src, &[(NoOperator, (3, 11))]);
}

/// GIVEN an optional struct at a place. WHEN a `.` or a `?.` write goes through it. THEN the
/// help "replace the value" comes only when that place can change: a `let mut` root, and a
/// `mut` field at each class step.
#[test]
fn replace_help_should_name_only_a_place_that_can_change() {
    let program = |setup: &str, write: &str| {
        format!(
            "struct P {{ n: i64 }}\nstruct S {{ o: P? }}\nclass C {{ p: P? }}\nclass M {{ mut p: P? }}\nfn f(o: P?, c: C, m: M) {{\n    {setup}\n    {write}\n}}\nfn main() {{}}"
        )
    };
    let rows = [
        ("a parameter", "", "o.n = 1", (7, 5), false),
        ("a field with no mut", "", "c.p.n = 1", (7, 5), false),
        ("a mut field", "", "m.p.n = 1", (7, 5), true),
        ("let s", "let s = S(o: None)", "s.o.n = 1", (7, 5), false),
        (
            "let mut s",
            "let mut s = S(o: None)",
            "s.o.n = 1",
            (7, 5),
            true,
        ),
        (
            "let mut s, ?.",
            "let mut s = S(o: None)",
            "s.o?.n = 1",
            (7, 8),
            true,
        ),
        ("a field with no mut, ?.", "", "c.p?.n = 1", (7, 8), false),
    ];
    for (row, setup, write, pos, help) in rows {
        let d = only_error(row, &program(setup, write), MayBeNone, pos);
        assert_eq!(d.help().is_some(), help, "row `{row}`: {d:?}");
    }
}

/// GIVEN `struct In { n: i64; os: [In?]; xs: [i64] }`. WHEN a write indexes an optional array
/// of optional structs after a `?.`. THEN it is one report: E0062 at the first `?.` with no help
/// with `let mut i`, E0001 with `let i`, at any depth of `.os[0]` steps after it. A `mut`
/// argument is one E0062 at the indexed array, with no help that names an unwrap.
#[test]
fn optional_struct_element_under_a_write_should_report_once() {
    let program = |binding: &str, write: &str| {
        format!(
            "struct In {{ n: i64; os: [In?]; xs: [i64] }}\nfn add(mut a: i64, b: i64) {{\n    a += b\n}}\nfn main() {{\n    {binding} = In(n: 1, os: [], xs: [])\n    {write}\n}}"
        )
    };
    let writes = [
        "i.os[0]?.os[0]?.n = 1",
        "i.os[0]?.os[0].n = 1",
        "i.os[0]?.os[0]?.n += 1",
        "i.os[0]?.os[0].n += 1",
        "i.os[0]?.os[0]?.xs.push(1)",
        "i.os[0]?.os[0].xs.push(1)",
        "i.os[0]?.os[0]?.xs?.push(1)",
        "i.os[0]?.os[0].xs?.push(1)",
    ];
    for write in writes {
        let d = only(write, &program("let mut i", write), MayBeNone, (7, 12));
        assert!(d.help().is_none(), "row `{write}`: {d:?}");
        only(write, &program("let i", write), AssignToImmutable, (7, 5));
    }
    for write in [
        "add(mut i.os[0]?.os[0]?.n, 1)",
        "add(mut i.os[0]?.os[0].n, 1)",
    ] {
        for binding in ["let mut i", "let i"] {
            let d = only(write, &program(binding, write), MayBeNone, (7, 13));
            assert!(d.help().is_none(), "row `{write}`: {d:?}");
        }
    }
    for depth in [3, 5, 10] {
        let path = format!("i.os[0]?{}", ".os[0]".repeat(depth - 1));
        for end in [".n = 1", ".n += 1", ".xs[0] = 1", "?.n = 1", ".xs.push(1)"] {
            let write = format!("{path}{end}");
            let d = only(&write, &program("let mut i", &write), MayBeNone, (7, 12));
            assert!(d.help().is_none(), "row `{write}`: {d:?}");
            only(&write, &program("let i", &write), AssignToImmutable, (7, 5));
        }
        let write = format!("add(mut {path}.n, 1)");
        for binding in ["let mut i", "let i"] {
            let d = only(&write, &program(binding, &write), MayBeNone, (7, 13));
            assert!(d.help().is_none(), "row `{write}`: {d:?}");
        }
    }
    let d = only(
        "i.os[0]?.n",
        &program("let mut i", "i.os[0]?.n = 1"),
        MayBeNone,
        (7, 12),
    );
    assert!(d.help().is_some(), "{d:?}");
}

/// GIVEN `class In` with a field `mut os: [In?]`. WHEN a write goes through `?.` and then
/// through two `.os[0]` steps, each of which gives an optional. THEN each of the two `.` steps
/// on an optional is E0062: the chain of a write skips a failed base only under a struct, so
/// this class form reports as today.
#[test]
fn optional_class_element_under_a_write_should_report_each_step() {
    let src = "class In {\n    mut n: i64\n    mut os: [In?]\n}\nfn main() {\n    let i = In(n: 1, os: [])\n    i.os[0]?.os[0].os[0].n = 1\n}";
    codes_row(
        "i.os[0]?.os[0].os[0].n",
        src,
        &[(MayBeNone, (7, 5)), (MayBeNone, (7, 5))],
    );
}

/// GIVEN a struct `P` or a class `P`. WHEN an `extend` block names `P?`, by itself or through an
/// alias. THEN it is E0155, as for any optional type.
#[test]
fn extend_of_an_optional_should_be_e0155() {
    for kw in ["struct", "class"] {
        let rows = [
            (
                "an alias of `P?`",
                format!("{kw} P {{ x: i64 }}\ntype A = P?\nextend A {{}}\nfn main() {{}}"),
                vec![(CannotExtend, (3, 8))],
            ),
            (
                "`P?`",
                format!("{kw} P {{ x: i64 }}\nextend P? {{}}\nfn main() {{}}"),
                vec![(CannotExtend, (2, 8))],
            ),
        ];
        error_rows(&rows);
    }
}

/// GIVEN `struct S { s: string }`, and `struct S { c: C }` with a class `C`. WHEN `is` compares
/// two values of `S`. THEN it is E0046 at the left operand: a struct is a value, and has no
/// object for `is` to compare, even when a counted field makes it counted.
#[test]
fn is_on_a_struct_should_be_e0046() {
    let rows = [
        (
            "a string field",
            "struct S { s: string }\nfn main() {\n    print(S(s: \"a\") is S(s: \"a\"))\n}",
            (3, 11),
        ),
        (
            "a class field",
            "class C {}\nstruct S { c: C }\nfn main() {\n    print(S(c: C()) is S(c: C()))\n}",
            (4, 11),
        ),
    ];
    for (row, src, at) in rows {
        codes_row(row, src, &[(IsNeedsObjects, at)]);
    }
}

/// GIVEN `struct S { f: <field> }`. WHEN `main` compares two values of `S`. THEN a field that has
/// `==` gives no diagnostic. An `any I` field and a class with no `Eq` are E0159 at the left
/// operand. So is a field of a struct that holds one, also through an array or an optional.
#[test]
fn struct_equality_should_follow_its_field_types() {
    let decls = "enum E {\n    A\n    B\n}\nstruct In { n: i64 }\ninterface I {\n    fn n(self) -> i64\n}\nclass Q {}\n";
    let compare = |field: &str| {
        format!(
            "{decls}struct S {{ f: {field} }}\nfn same(a: S, b: S) -> bool {{\n    return a == b\n}}\nfn main() {{}}"
        )
    };
    let ok = [
        ("i64", "a scalar"),
        ("string", "a string"),
        ("[i64]", "an array"),
        ("E", "an enum"),
        ("In", "a struct"),
        ("i64?", "an optional"),
        ("[S]", "an array of the struct itself"),
        ("[In]?", "an optional array of structs"),
    ];
    for (field, row) in ok {
        codes_row(row, &compare(field), &[]);
    }
    let bad = [
        ("any I", "no `==`"),
        ("Q", "a class with no `Eq`"),
        ("[any I]", "an array of values with no `==`"),
        ("(any I)?", "an optional of a value with no `==`"),
    ];
    for (field, row) in bad {
        let src = compare(field);
        codes_row(row, &src, &[(NoOperator, (12, 12))]);
    }
    let nested = format!(
        "{decls}struct W {{ a: any I }}\nstruct S {{ w: W }}\nfn main() {{\n    print(S(w: W(a: x())) == S(w: W(a: x())))\n}}\nfn x() -> any I {{\n    return Z()\n}}\nclass Z: I {{\n    fn n(self) -> i64 {{ 1 }}\n}}"
    );
    codes_row(
        "a struct inside a struct",
        &nested,
        &[(NoOperator, (13, 11))],
    );
}

/// GIVEN a struct that holds itself, directly, through another struct or through an enum. WHEN
/// the program is checked. THEN it is E0027 when no value of it can be built, at its first enum,
/// or else at its first struct. These end the cycle, so each has no error: an optional, an array,
/// an enum's variant that needs no `S`, and the variant `N` of `W<S>`, which holds no `S`.
#[test]
fn struct_cycle_should_need_a_way_out() {
    let rows = [
        (
            "direct",
            "struct S { s: S }\nfn main() {}".to_string(),
            vec![(NeverConstructed, (1, 8))],
        ),
        (
            "mutual",
            "struct A { b: B }\nstruct B { a: A }\nfn main() {}".to_string(),
            vec![(NeverConstructed, (1, 8))],
        ),
        (
            "an optional ends it",
            "struct S { s: S? }\nfn main() {}".to_string(),
            vec![],
        ),
        (
            "an array ends it",
            "struct S { xs: [S] }\nfn main() {}".to_string(),
            vec![],
        ),
        (
            "an enum's variant ends it",
            "struct S { e: E }\nenum E {\n    A(s: S)\n    B\n}\nfn main() {}".to_string(),
            vec![],
        ),
        (
            "an enum with no way out",
            "struct S { e: E }\nenum E {\n    A(s: S)\n}\nfn main() {}".to_string(),
            vec![(NeverConstructed, (2, 6))],
        ),
        (
            "the variant `N` that holds no `S` ends it",
            "enum W<T> {\n    A(x: T)\n    N\n}\nstruct S { w: W<S> }\nfn main() {}".to_string(),
            vec![],
        ),
        (
            "an enum's type argument with no way out",
            "enum W<T> {\n    A(x: T)\n}\nstruct S { w: W<S> }\nfn main() {}".to_string(),
            vec![(NeverConstructed, (4, 8))],
        ),
        (
            "a class and a struct",
            "class C { s: S }\nstruct S { c: C }\nfn main() {}".to_string(),
            vec![(NeverConstructed, (2, 8))],
        ),
    ];
    error_rows(&rows);
}

/// GIVEN `let mut t` that holds a struct. WHEN the program writes a field of `t` and never reads
/// `t`. THEN it is W0014, and not W0002: the write is a change of `t`. A write whose index
/// reads `t` is a read, so it has no W0014.
#[test]
fn struct_write_should_keep_its_w0014() {
    judge_unused();
    codes_row(
        "a field write",
        "struct P { n: i64 }\nfn main() {\n    let mut t = P(n: 1)\n    t.n = 1\n}",
        &[(UnusedVariable, (3, 13))],
    );
    codes_row(
        "an index that reads the root",
        "struct A { a: i64 }\nstruct P { n: i64; xs: [A] }\nfn main() {\n    let mut t = P(n: 0, xs: [A(a: 0)])\n    t.xs[t.n].a = 1\n}",
        &[],
    );
}

/// GIVEN a generic struct whose type argument grows: in its own field, or through the generated
/// `==` or `print` of a struct that a written method of another struct builds. WHEN the program is
/// checked. THEN each is E0140, at the place that the same program of enums reports.
#[test]
fn growing_generic_struct_should_be_e0140() {
    let eq = "struct S<T> { v: T }\nstruct K<T> { v: T }\nextend<T: Eq> K<T>: Eq {\n    fn eq(self, o: K<T>) -> bool { S(v: K(v: [self.v])) == S(v: K(v: [o.v])) }\n}\nfn main() { print(K(v: 1) == K(v: 1)) }";
    let display = "struct S<T> { v: T }\nstruct K<T> { v: T }\nextend<T: Display> K<T>: Display {\n    fn to_string(self) -> string { \"{S(v: K(v: [self.v]))}\" }\n}\nfn main() { print(K(v: 1)) }";
    let rows = [
        (
            "its own field",
            "struct G<T> { v: T; next: G<[T]>? }\nfn main() {\n    print(G(v: 1, next: None))\n}"
                .to_string(),
            vec![(InstantiationCycle, (1, 27))],
        ),
        (
            "a generated `==`",
            eq.to_string(),
            vec![(InstantiationCycle, (4, 36))],
        ),
        (
            "a generated `print`",
            display.to_string(),
            vec![(InstantiationCycle, (4, 38))],
        ),
        (
            "an enum's generated `==`",
            eq.replace("struct S<T> { v: T }", "enum S<T> { A(v: T) }")
                .replace("S(v:", "S.A(v:"),
            vec![(InstantiationCycle, (4, 36))],
        ),
    ];
    error_rows(&rows);
}

/// GIVEN `struct V { n: i64 }`, `struct W { v: V }`, and a written `eq` of `V` that compares two
/// values of `W`, whose generated `eq` calls the written `eq` of `V`. WHEN the program is checked.
/// THEN it reports what the same program of enums reports: nothing.
#[test]
fn eq_through_a_generated_eq_should_be_a_cycle() {
    let main = "fn main() {\n    print(V.A(n: 1) == V.A(n: 1))\n}";
    let enums = format!(
        "enum V {{\n    A(n: i64)\n}}\nenum W {{\n    B(v: V)\n}}\nextend V: Eq {{\n    fn eq(self, o: V) -> bool {{\n        W.B(v: self) == W.B(v: o)\n    }}\n}}\n{main}"
    );
    let structs = "struct V { n: i64 }\nstruct W { v: V }\nextend V: Eq {\n    fn eq(self, o: V) -> bool {\n        W(v: self) == W(v: o)\n    }\n}\nfn main() {\n    print(V(n: 1) == V(n: 1))\n}";
    codes_row("the enums", &enums, &[]);
    codes_row("the structs", structs, &[]);
}

/// GIVEN `struct Pair<T> { a: T; b: T }`. WHEN `main` compares two values of `Pair<i64>`, and two
/// of `Pair<any I>`. THEN the first has `==`, and the second is E0159 at the left operand. The
/// generated `Eq` of a struct asks each field type for it.
#[test]
fn conditional_struct_equality_should_follow_its_argument() {
    let decls = "struct Pair<T> { a: T; b: T }\ninterface I {\n    fn n(self) -> i64\n}\nclass Z: I {\n    fn n(self) -> i64 { 1 }\n}\nfn z() -> any I {\n    Z()\n}\n";
    codes_row(
        "Pair<i64>",
        &format!("{decls}fn main() {{\n    print(Pair(a: 1, b: 2) == Pair(a: 1, b: 2))\n}}"),
        &[],
    );
    codes_row(
        "Pair<any I>",
        &format!(
            "{decls}fn main() {{\n    let p: Pair<any I> = Pair(a: z(), b: z())\n    print(p == p)\n}}"
        ),
        &[(NoOperator, (13, 11))],
    );
}

/// GIVEN `struct P { n: i64 }` with `extend P { fn m(mut self) {} }`. WHEN the program is
/// checked. THEN it is E0018 at the `mut`, with a help and no fix, as the method of a struct's
/// own body has: a class's form has a fix that removes the `mut`. A body that changes `self` or
/// a part of it gives no E0001 after the E0018: the body is checked as `mut self` asks.
#[test]
fn mut_self_on_a_struct_should_be_e0018() {
    let changes = |body: &str| {
        format!(
            "struct In {{ n: i64 }}\nfn bump(mut x: i64) {{ x = 1 }}\n\
             struct P {{ n: i64; xs: [i64]; i: In; o: In? }}\n\
             extend P {{\n    fn m(mut self) {{ {body} }}\n}}\nfn main() {{}}"
        )
    };
    let rows = [
        (
            "an extend of a struct",
            "struct P { n: i64 }\nextend P {\n    fn m(mut self) {}\n}\nfn main() {}".to_string(),
        ),
        (
            "an extend of an alias of a struct",
            "struct P { n: i64 }\ntype A = P\nextend A {\n    fn m(mut self) {}\n}\nfn main() {}"
                .to_string(),
        ),
        ("a write of a field of `self`", changes("self.n = 2")),
        ("a `push` to a field of `self`", changes("self.xs.push(1)")),
        (
            "a write of a nested field of `self`",
            changes("self.i.n = 2"),
        ),
        (
            "a field of `self` as a `mut` argument",
            changes("bump(mut self.n)"),
        ),
    ];
    for (row, src) in &rows {
        let (row, src) = (*row, src.as_str());
        let d = only_error(row, src, MutSelf, (src.lines().count() - 2, 10));
        assert!(d.fixes().is_empty(), "row `{row}`: {d:?}");
        assert!(d.help().is_some(), "row `{row}`: {d:?}");
    }
    // A write through `?.` stays E0062, as for a `let mut` root, and is no E0001.
    codes_row(
        "a write through `?.` of a field of `self`",
        &changes("self.o?.n = 2"),
        &[(MutSelf, (5, 10)), (MayBeNone, (5, 28))],
    );
}

/// GIVEN a struct whose declaration lists `K` but has no `k`, and one that lists `Ord`. WHEN the
/// program is checked. THEN each is E0152 at the interface in the list. `E0152-struct-ord` pins
/// the label of the `Ord` row.
#[test]
fn struct_conformance_list_failure_should_be_e0152() {
    let rows = [
        (
            "a list without the method",
            "interface K {\n    fn k(self) -> i64\n}\nstruct P: K { x: i64 }\nfn main() {\n    print(P(x: 1).x)\n}",
            (4, 11),
        ),
        (
            "a list with `Ord`",
            "struct N: Ord { n: i64 }\nfn main() {\n    print(N(n: 1).n)\n}",
            (1, 11),
        ),
    ];
    for (row, src, pos) in rows {
        only_error(row, src, Code::ConformanceBody, pos);
    }
}

/// GIVEN a program that breaks the rule of a page that a struct now reaches. It has a class, or
/// a struct in its place. WHEN each is checked. THEN the struct gives the class's code at the
/// same place: one row for each page.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one row for each page, so the table reads in one place"
)]
fn struct_bound_failure_should_report_each_code() {
    use Code::{
        Ambiguous, ConformanceBody, ConformsTwice, FieldComma, FieldLet, FieldNotMethod,
        MethodNeedsSelf, MethodNotField, NotConform, SelfNotFirst, SelfOutsideMethod,
    };
    // Each row: its page, its program with `KW` for the keyword, its code, and where the class
    // and the struct report it. The two differ when the report is on a line with the keyword.
    type Row<'a> = (&'a str, &'a str, Code, (usize, usize), (usize, usize));
    let table: [Row<'_>; 19] = [
        (
            "E0015",
            "KW P { x: i64, y: i64 }\nfn main() {\n    print(P(x: 1, y: 2).x)\n}",
            FieldComma,
            (1, 17),
            (1, 18),
        ),
        (
            "E0016",
            "KW P {\n    let x: i64\n}\nfn main() {\n    print(P(x: 1).x)\n}",
            FieldLet,
            (2, 5),
            (2, 5),
        ),
        (
            "E0017",
            "KW P {\n    x: i64\n    fn m() {}\n}\nfn main() {\n    print(P(x: 1).x)\n}",
            MethodNeedsSelf,
            (3, 8),
            (3, 8),
        ),
        (
            "E0020",
            "KW P {\n    x: i64\n    fn m(a: i64, self) {}\n}\nfn main() {\n    print(P(x: 1).x)\n}",
            SelfNotFirst,
            (3, 18),
            (3, 18),
        ),
        (
            "E0034",
            "KW P { x: i64 }\nextend P {\n    fn x(self) {}\n}\nfn main() {\n    print(P(x: 1).x)\n}",
            DefinedTwice,
            (3, 8),
            (3, 8),
        ),
        (
            "E0041",
            "KW P { x: i64 }\nextend P {\n    fn m(self) {}\n}\nfn show() {\n    print(self.x)\n}\nfn main() {\n    show()\n    P(x: 1).m()\n}",
            SelfOutsideMethod,
            (6, 11),
            (6, 11),
        ),
        (
            "E0057",
            "KW P { x: i64 }\nextend P {\n    fn m(self) -> i64 { self.x }\n}\nfn main() {\n    print(P(x: 1).m)\n}",
            MethodNotField,
            (6, 19),
            (6, 19),
        ),
        (
            "E0058",
            "KW P { x: i64 }\nfn main() {\n    print(P(x: 1).x())\n}",
            FieldNotMethod,
            (3, 19),
            (3, 19),
        ),
        (
            "E0070",
            "KW P { x: i64 }\nextend P {\n    fn m(self) -> i64 { self.x }\n}\nfn main() {\n    print(m(P(x: 1)))\n}",
            MethodAsFunction,
            (6, 11),
            (6, 11),
        ),
        (
            "E0148, a conformance list",
            "KW Q { x: i64 }\nKW P: Q { x: i64 }\nfn main() {\n    print(P(x: 1).x + Q(x: 1).x)\n}",
            NotAnInterface,
            (2, 10),
            (2, 11),
        ),
        (
            "E0148, a bound",
            "KW Q { x: i64 }\nfn f<T: Q>(t: T) {}\nfn main() {\n    print(Q(x: 1).x)\n}",
            NotAnInterface,
            (2, 9),
            (2, 9),
        ),
        (
            "E0152, a conformance list",
            "interface Shape {\n    fn area(self) -> f64\n}\nKW P: Shape { r: f64 }\nfn main() {\n    print(P(r: 1.0).r)\n}",
            ConformanceBody,
            (4, 10),
            (4, 11),
        ),
        (
            "E0152, an extend",
            "interface Shape {\n    fn area(self) -> f64\n}\nKW P { r: f64 }\nextend P: Shape {}\nfn main() {\n    print(P(r: 1.0).r)\n}",
            ConformanceBody,
            (5, 11),
            (5, 11),
        ),
        (
            "E0153",
            "KW P: Display { x: i64 }\nextend P: Display {\n    fn to_string(self) -> string { \"p\" }\n}\nfn main() {\n    print(P(x: 1))\n}",
            ConformsTwice,
            (2, 11),
            (2, 11),
        ),
        (
            "E0154, an inherited interface",
            "interface Named {\n    fn name(self) -> string\n}\ninterface Shape: Named {\n    fn area(self) -> f64\n}\nKW P { r: f64 }\nextend P: Shape {\n    fn area(self) -> f64 { self.r }\n}\nfn main() {\n    print(P(r: 1.0).area())\n}",
            NotConform,
            (8, 11),
            (8, 11),
        ),
        (
            "E0154, a bound of the type",
            "interface K {\n    fn k(self) -> i64\n}\nKW P<T: K> { t: T }\nfn main() {\n    let p = P(t: 1)\n    print(p.t)\n}",
            NotConform,
            (6, 18),
            (6, 18),
        ),
        (
            "E0155",
            "KW P { x: i64 }\nextend<T> P {\n    fn m(self) {}\n}\nfn main() {\n    P(x: 1).m()\n}",
            CannotExtend,
            (2, 8),
            (2, 8),
        ),
        (
            "E0156",
            "interface A {\n    fn name(self) -> string\n}\ninterface B {\n    fn name(self) -> string\n}\nKW P { x: i64 }\nextend P: A {\n    fn name(self) -> string { \"a\" }\n}\nextend P: B {\n    fn name(self) -> string { \"b\" }\n}\nfn main() {\n    let n = P(x: 1).name()\n    print(n)\n}",
            Ambiguous,
            (15, 21),
            (15, 21),
        ),
        (
            "E0159, a written `Eq` whose bound fails",
            "class Q {}\nKW Pair<T> { a: T }\nextend<T: Ord> Pair<T>: Eq {\n    fn eq(self, o: Pair<T>) -> bool {\n        self.a < o.a\n    }\n}\nfn main() {\n    print(Pair(a: Q()) == Pair(a: Q()))\n}",
            NoOperator,
            (9, 11),
            (9, 11),
        ),
    ];
    let mut rows = Vec::new();
    for (page, src, code, class_at, struct_at) in table {
        for (kw, pos) in [("class", class_at), ("struct", struct_at)] {
            rows.push((
                format!("{page}, a {kw}"),
                src.replace("KW", kw),
                vec![(code, pos)],
            ));
        }
    }
    let rows: Vec<ErrorRow<'_>> = (rows.iter())
        .map(|(name, src, wanted)| (name.as_str(), src.clone(), wanted.clone()))
        .collect();
    error_rows(&rows);
}

/// `==` on a struct runs the written `eq` of a class that it holds, directly, through an enum or
/// through a type argument. A call in the body could change what that `eq` reads. A struct that
/// holds the poison type gets no W0011 either. A struct of numbers gets W0011.
#[test]
fn struct_that_holds_a_class_should_not_get_w0011() {
    use Code::{UnchangedLoopCondition, UnknownType};
    let node = "class Node: Eq {\n    v: i64\n    fn eq(self, other: Self) -> bool { self.v == other.v }\n}\n";
    let tail = "fn f(a: S, b: S) {\n    while a == b { print(1) }\n}";
    for (row, decls, wanted) in [
        ("a class field", "struct S { n: Node }", vec![]),
        (
            "an enum field that holds a class",
            "enum E {\n    V(Node)\n}\nstruct S { e: E }",
            vec![],
        ),
        (
            "an enum payload that holds the struct",
            "struct T { n: [Node] }\nenum S {\n    V(T)\n}",
            vec![],
        ),
        (
            "a type argument",
            "struct G<T> { t: T? }\nstruct S { g: G<Node> }",
            vec![],
        ),
        (
            "an unknown type",
            "struct S { n: Fooo }",
            vec![(UnknownType, (5, 15))],
        ),
        (
            "numbers",
            "struct S { n: i64 }",
            vec![(UnchangedLoopCondition, (7, 11))],
        ),
    ] {
        codes_row(
            row,
            &format!("{node}{decls}\n{tail}\nfn main() {{}}"),
            &wanted,
        );
    }
}

/// A `push` of a struct's own method writes nothing to its receiver, as for an enum, so the
/// `let mut` gets W0002.
#[test]
fn own_push_of_a_struct_should_not_change_its_receiver() {
    codes(
        "struct S { n: i64 }\nextend S {\n    fn push(self, x: i64) { print(x + self.n) }\n}\nfn main() {\n    let mut t = S(n: 1)\n    t.push(3)\n}",
        &[(NeverReassigned, (6, 13))],
    );
}
