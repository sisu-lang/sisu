//! E0043 and E0031 say why a value is missing: an `if` with no `else`, or a call of a function
//! that declares no return type. #637, items 1 to 3.

use super::*;

/// The declarations that the rows use. Line 1 holds `greet`, line 4 the `hi` of `S`, line 7 the
/// `hi` of `E`.
const DECLS: &str = "fn greet() { print(1) }
fn gg<T>(x: T) {}
struct S { n: i64
    fn hi(self) {}
}
enum E { A
    fn hi(self) {}
}
interface U { fn m(self) }
fn g(x: i64) {}
";

/// A place in a program: the `nth` match of a text, or the `}` that ends the function before
/// `main`.
#[derive(Clone, Copy)]
enum Place {
    Text(&'static str, usize),
    Close,
}

/// Why the expression has no value, as the shape of the report shows it. The fixtures
/// `E0043-*` pin the wording of each cause.
enum Cause {
    /// No cause: the message only.
    Plain,
    /// A value that never finishes: a label.
    Never,
    /// An `if` or `if let` with no `else`: a help.
    NoElse,
    /// A call of a function that declares no return type: a secondary label at the
    /// declaration, when the function has one in the program.
    Call(Option<Place>),
}

/// A program with one E0043, and what the report says.
struct Row {
    id: &'static str,
    /// Declarations after `DECLS`, each ending in a line break.
    extra: &'static str,
    /// The lines of `main` after `let c = true` and `let o: i64? = None`.
    body: &'static str,
    at: Place,
    cause: Cause,
}

/// A program with one E0031, and what the report says.
struct BodyRow {
    id: &'static str,
    /// Declarations after `DECLS`, ending with the function `f`, each ending in a line break.
    extra: &'static str,
    /// The tail expression, which a secondary label points at.
    tail: Option<Place>,
    /// Whether the report has a help: only for an `if` with no `else`.
    help: bool,
}

const fn row(id: &'static str, body: &'static str, at: Place, cause: Cause) -> Row {
    Row {
        id,
        extra: "",
        body,
        at,
        cause,
    }
}

const fn row_with(
    id: &'static str,
    extra: &'static str,
    body: &'static str,
    (at, cause): (Place, Cause),
) -> Row {
    Row {
        id,
        extra,
        body,
        at,
        cause,
    }
}

const fn text(text: &'static str, nth: usize) -> Place {
    Place::Text(text, nth)
}

/// A call of a function whose declaration is at `declared`.
const fn call(declared: Option<Place>) -> Cause {
    Cause::Call(declared)
}

const GREET: Cause = call(Some(Place::Text("greet", 0)));

#[expect(clippy::too_many_lines, reason = "one row for each case of the table")]
fn value_rows() -> Vec<Row> {
    let (m, g) = (
        "fn bump(mut n: i64) { n = 1 }\n",
        "struct Q { n: i64\n    fn bump(self, mut k: i64) { k = 1 }\n}\n",
    );
    vec![
        row(
            "R1",
            "let x = if c { 5 }",
            text("if c { 5 }", 0),
            Cause::NoElse,
        ),
        row(
            "R2",
            "let x: i64 = if c { print(1) }",
            text("if c { print(1) }", 0),
            Cause::NoElse,
        ),
        row(
            "R3",
            "let x = if let Some(v) = o { v }",
            text("if let Some(v) = o { v }", 0),
            Cause::NoElse,
        ),
        row("R4", "g(if c { 5 })", text("if c { 5 }", 0), Cause::NoElse),
        row(
            "R5",
            "let a = [if c { 5 }]",
            text("if c { 5 }", 0),
            Cause::NoElse,
        ),
        row(
            "R6",
            "let a = (if c { 5 }) + 1",
            text("(if c { 5 })", 0),
            Cause::NoElse,
        ),
        row(
            "R7",
            "let a = loop { break if c { 5 } }",
            text("if c { 5 }", 0),
            Cause::NoElse,
        ),
        row_with(
            "R8",
            "fn f(c: bool) -> i64 { return if c { 5 } }\n",
            "",
            (text("if c { 5 }", 0), Cause::NoElse),
        ),
        row_with(
            "R9",
            "fn f(c: bool) -> i64 { return if c { print(1) } }\n",
            "",
            (text("if c { print(1) }", 0), Cause::NoElse),
        ),
        row_with(
            "R32",
            "fn f(c: bool) { return if c { 5 } }\n",
            "",
            (text("if c { 5 }", 0), Cause::NoElse),
        ),
        row_with(
            "R33",
            "fn f(c: bool) -> i64 { return if c { return 1 } }\n",
            "",
            (text("if c { return 1 }", 0), Cause::NoElse),
        ),
        // The join does not know the `return`: it keeps the help of the `if`.
        row_with(
            "R34",
            "fn f(c: bool) { return if c { 5 } else if !c { 6 } }\n",
            "",
            (text("if !c { 6 }", 0), Cause::NoElse),
        ),
        row_with(
            "R10",
            "fn f(o: i64?) -> i64 { return if let Some(v) = o { print(v) } }\n",
            "",
            (text("if let Some(v) = o { print(v) }", 0), Cause::NoElse),
        ),
        row("R11", "let x: i64 = greet()", text("greet()", 1), GREET),
        row("R12", "g(greet())", text("greet()", 1), GREET),
        row("R13", "let a = greet() + 1", text("greet()", 1), GREET),
        row(
            "R14",
            "let mut x = 1\nx = greet()",
            text("greet()", 1),
            GREET,
        ),
        row_with(
            "R15",
            "fn f() -> i64 { return greet() }\n",
            "",
            (text("greet()", 1), GREET),
        ),
        row_with(
            "R16",
            "fn f() -> i64? { return greet() }\n",
            "",
            (text("greet()", 1), GREET),
        ),
        row(
            "R17",
            "let x: i64 = S(n: 1).hi()",
            text("S(n: 1).hi()", 0),
            call(Some(text("hi", 0))),
        ),
        row(
            "R18",
            "let x: i64 = E.A.hi()",
            text("E.A.hi()", 0),
            call(Some(text("hi", 1))),
        ),
        row(
            "R19",
            "let x: i64 = gg(1)",
            text("gg(1)", 0),
            call(Some(text("gg", 0))),
        ),
        row(
            "R20",
            "let x: i64 = print(1)",
            text("print(1)", 1),
            call(None),
        ),
        row(
            "R21",
            "let x = match c { true => 1; false => greet() }",
            text("greet()", 1),
            GREET,
        ),
        row("R22", "let x = o ?? greet()", text("greet()", 1), GREET),
        row(
            "R23",
            "let a: i64 = loop { break greet() }",
            text("greet()", 1),
            GREET,
        ),
        row_with(
            "R24",
            "fn f<T: U>(t: T) { let x: i64 = t.m() }\n",
            "",
            (text("t.m()", 0), call(None)),
        ),
        row(
            "R25",
            "let x: i64 = { greet() }",
            text("{ greet() }", 0),
            Cause::Plain,
        ),
        row(
            "R26",
            "let x: i64 = if c { greet() } else { greet() }",
            text("if c { greet() } else { greet() }", 0),
            Cause::Plain,
        ),
        row(
            "R27",
            "let x = if c { return } else { return }",
            text("if c { return } else { return }", 0),
            Cause::Never,
        ),
        row_with(
            "R28",
            m,
            "let mut n = 1\nlet x: i64 = bump(mut n)",
            (text("bump(mut n)", 0), call(Some(text("bump", 0)))),
        ),
        row_with(
            "R29",
            g,
            "let mut n = 1\nlet y: i64 = Q(n: 1).bump(mut n)",
            (text("Q(n: 1).bump(mut n)", 0), call(Some(text("bump", 0)))),
        ),
        row(
            "R30",
            "let d = false\nlet x: i64 = if c { print(1) } else if d { print(2) }",
            text("if c { print(1) } else if d { print(2) }", 0),
            Cause::NoElse,
        ),
        row_with(
            "R31",
            "fn f(c: bool, d: bool) -> i64 { return if c { print(1) } else if d { print(2) } }\n",
            "",
            (
                text("if c { print(1) } else if d { print(2) }", 0),
                Cause::NoElse,
            ),
        ),
        // An `else` block with two statements is no `else if`.
        row(
            "R38",
            "let x: i64 = if c { greet() } else { greet(); greet() }",
            text("if c { greet() } else { greet(); greet() }", 0),
            Cause::Plain,
        ),
        // A braced `else` that holds one `if` is no `else if`: the outer `if` has an `else`.
        row(
            "R39",
            "let x: i64 = if c { greet() } else { if c { greet() } }",
            text("if c { greet() } else { if c { greet() } }", 0),
            Cause::Plain,
        ),
        row(
            "R40",
            "let x: i64 = if c { greet() } else { if let Some(v) = o { print(v) } }",
            text(
                "if c { greet() } else { if let Some(v) = o { print(v) } }",
                0,
            ),
            Cause::Plain,
        ),
        row_with(
            "R41",
            "fn f(c: bool) -> i64 { return if c { print(1) } else { if c { print(2) } } }\n",
            "",
            (
                text("if c { print(1) } else { if c { print(2) } }", 0),
                Cause::Plain,
            ),
        ),
        // The join reports the inner `if`, which has no `else`.
        row(
            "R42",
            "let x = if c { 1 } else { if c { 2 } }",
            text("if c { 2 }", 0),
            Cause::NoElse,
        ),
        // An argument of a generic call: the cause shows at each place that checks one.
        row_with(
            "R43",
            "fn consume<T>(x: T, y: T) {}\n",
            "consume(if c { 5 }, 1)",
            (text("if c { 5 }", 0), Cause::NoElse),
        ),
        row_with(
            "R44",
            "fn opt<T>(x: [T?]) {}\n",
            "opt([if c { 5 }])",
            (text("if c { 5 }", 0), Cause::NoElse),
        ),
        // A written `-> unit`: the secondary label points at it.
        row_with(
            "R45",
            "fn gu() -> unit {}\n",
            "let x: i64 = gu()",
            (text("gu()", 1), call(Some(text("unit", 0)))),
        ),
        row_with(
            "R37",
            "struct W: U { fn m(self) {} }\n",
            "let u: any U = W()\nlet x: i64 = u.m()",
            (text("u.m()", 0), call(None)),
        ),
    ]
}

fn body_rows() -> Vec<BodyRow> {
    let rows = [
        (
            "B1",
            "fn five(c: bool) -> i64 {\n    if c { 5 }\n}\n",
            Some(text("if c { 5 }", 0)),
            true,
        ),
        (
            "B2",
            "fn f(o: i64?) -> i64 { if let Some(v) = o { v } }\n",
            Some(text("if let Some(v) = o { v }", 0)),
            true,
        ),
        (
            "B3",
            "fn f() -> i64 { greet() }\n",
            Some(text("greet()", 1)),
            false,
        ),
        (
            "B4",
            "fn f() -> i64 { S(n: 1).hi() }\n",
            Some(text("S(n: 1).hi()", 0)),
            false,
        ),
        ("B5", "fn f() -> i64 { let n = 5 }\n", None, false),
        (
            "B6",
            "type N = i64\nfn f(c: bool) -> N { if c { 5 } }\n",
            Some(text("if c { 5 }", 0)),
            true,
        ),
        (
            "B7",
            "fn f(c: bool) -> bool { if c { true } }\n",
            Some(text("if c { true }", 0)),
            true,
        ),
        (
            "B8",
            "fn f(c: bool) -> u8 { if c { 1 } }\n",
            Some(text("if c { 1 }", 0)),
            true,
        ),
        (
            "B9",
            "fn h(c: bool, d: bool) -> i64 { if c { print(1) } else if d { print(2) } }\n",
            Some(text("if c { print(1) } else if d { print(2) }", 0)),
            true,
        ),
        (
            "B10",
            "fn f(c: bool) -> i64 { if c { return 5 } }\n",
            Some(text("if c { return 5 }", 0)),
            true,
        ),
        (
            "B11",
            "fn f(c: bool) -> i64 { if c { greet() } else { if c { greet() } } }\n",
            None,
            false,
        ),
    ];
    rows.into_iter()
        .map(|(id, extra, tail, help)| BodyRow {
            id,
            extra,
            tail,
            help,
        })
        .collect()
}

/// The line, the column and the text of `place` in `src`.
fn locate(src: &str, place: Place) -> (usize, usize, &'static str) {
    let (offset, shown) = match place {
        Place::Text(shown, nth) => {
            let (offset, _) = src
                .match_indices(shown)
                .nth(nth)
                .unwrap_or_else(|| panic!("match {nth} of `{shown}` in {src}"));
            (offset, shown)
        }
        Place::Close => (src.rfind("\nfn main").expect("a `main`") - 1, "}"),
    };
    let (line, col) = line_col(src, offset);
    (line, col, shown)
}

fn position(src: &str, place: Place) -> (usize, usize) {
    let (line, col, _) = locate(src, place);
    (line, col)
}

fn value_source(row: &Row) -> String {
    let body: String = row
        .body
        .lines()
        .map(|l| ["    ", l, "\n"].concat())
        .collect();
    format!(
        "{DECLS}{}fn main() {{\n    let c = true\n    let o: i64? = None\n{body}}}\n",
        row.extra
    )
}

fn body_source(row: &BodyRow) -> String {
    format!("{DECLS}{}fn main() {{}}\n", row.extra)
}

/// The E0043 of `row`, after the checks of its place; a row whose report has the wrong shape
/// is in `red`.
fn value_diagnostic(row: &Row, red: &mut Vec<String>) -> Diagnostic {
    let src = value_source(row);
    let secondary: Vec<_> = match &row.cause {
        Cause::Call(Some(place)) => vec![position(&src, *place)],
        _ => vec![],
    };
    let d = diagnostic_row(
        row.id,
        &src,
        Code::NoValue,
        locate(&src, row.at),
        &secondary,
    );
    let found = (d.label().is_some(), d.help().is_some());
    let want = (
        matches!(row.cause, Cause::Never),
        matches!(row.cause, Cause::NoElse),
    );
    if found != want {
        red.push(format!(
            "row `{}`: (label, help) found {found:?}, wanted {want:?}",
            row.id
        ));
    }
    d
}

/// The E0031 of `row`; a row whose report has the wrong shape is in `red`.
fn body_diagnostic(row: &BodyRow, red: &mut Vec<String>) -> Diagnostic {
    let src = body_source(row);
    let secondary: Vec<_> = row
        .tail
        .iter()
        .map(|&place| position(&src, place))
        .collect();
    let d = diagnostic_row(
        row.id,
        &src,
        Code::BodyHasNoValue,
        locate(&src, Place::Close),
        &secondary,
    );
    if d.help().is_some() != row.help {
        red.push(format!(
            "row `{}`: help found {}, wanted {}",
            row.id,
            d.help().is_some(),
            row.help
        ));
    }
    d
}

fn assert_no_red(red: &[String]) {
    assert!(
        red.is_empty(),
        "{} red rows:\n{}",
        red.len(),
        red.join("\n")
    );
}

/// GIVEN a value that has no value, because of an `if` with no `else` or a call of a function
///   that declares no return type
/// WHEN the checker reports it
/// THEN E0043 has a help for an `if`, and points at the declaration of the function, when
///   there is one; the `E0043-*` fixtures pin the text
#[test]
fn a_value_with_no_value_should_say_why() {
    let mut red = Vec::new();
    for row in value_rows() {
        value_diagnostic(&row, &mut red);
    }
    assert_no_red(&red);
}

/// The call of a function that takes a wrong count of arguments fails, so its report holds no
/// declaration.
#[test]
fn a_failed_call_with_no_value_should_point_at_no_declaration() {
    let src = format!("{DECLS}fn main() {{\n    let x: i64 = greet(1)\n}}\n");
    let (_, ds) = checked(&src);
    let found: Vec<_> = ds.iter().map(|d| (d.code(), at(&src, d.span()))).collect();
    assert_eq!(
        found,
        [(Code::ArgumentCount, (12, 18)), (Code::NoValue, (12, 18))]
    );
    let d = &ds[1];
    assert_eq!(d.secondary().len(), 0);
    assert!(d.help().is_none());
}

/// GIVEN a function whose body ends in an `if` with no `else`, a call with no value, or no
///   expression
/// WHEN the checker reports E0031
/// THEN a label points at the tail expression, and an `if` has a help; the `E0031-*` fixtures
///   pin the text
#[test]
fn a_body_with_no_value_should_label_its_tail() {
    let mut red = Vec::new();
    for row in body_rows() {
        body_diagnostic(&row, &mut red);
    }
    assert_no_red(&red);
}

/// The count of fixes of the one diagnostic of the program `src`.
fn fixes_of_one(id: &str, src: &str) -> usize {
    let (_, ds) = checked(src);
    let [d] = ds.as_slice() else {
        panic!("row `{id}`: {} diagnostics, wanted 1", ds.len());
    };
    d.fixes().len()
}

/// GIVEN the reports of rows R1, R9, R11 and B1
/// WHEN they are read
/// THEN none of them holds a fix
#[test]
fn a_unit_value_should_offer_no_fix() {
    let mut fixes: Vec<_> = value_rows()
        .iter()
        .filter(|row| ["R1", "R9", "R11"].contains(&row.id))
        .map(|row| (row.id, fixes_of_one(row.id, &value_source(row))))
        .collect();
    fixes.extend(
        body_rows()
            .iter()
            .filter(|row| row.id == "B1")
            .map(|row| (row.id, fixes_of_one(row.id, &body_source(row)))),
    );
    assert_eq!(fixes, [("R1", 0), ("R9", 0), ("R11", 0), ("B1", 0)]);
}
