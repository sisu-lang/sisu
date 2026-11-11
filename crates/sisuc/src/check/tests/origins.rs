//! The origin of an expected type: an E0032 says where its required type comes from, as a
//! secondary label, a note or a label at the primary span (S21). These tests read positions and
//! counts, never text.

use super::*;

/// How an E0032 explains its required type.
enum Explained {
    /// Exactly one secondary label, at this line and column, and no note.
    Label(u32, u32),
    /// Exactly one note, and no secondary label: the origin is in the prelude.
    Note,
    /// A label at the primary span, and no secondary label and no note: a language rule.
    Rule,
    /// No label of either kind and no note.
    Nothing,
}

/// The line and column of `span` in `src`.
fn position(src: &str, span: Span) -> (u32, u32) {
    let (line, col) = at(src, span);
    let narrow = |n: usize| u32::try_from(n).expect("a test program is short");
    (narrow(line), narrow(col))
}

/// For each row (name, program, primary position, explanation): the program gives one error, an
/// E0032 at that position, explained as the row says. Warnings do not count. A failure names
/// each red row.
fn origin_rows(rows: &[(&str, &str, (u32, u32), Explained)]) {
    let mut red = Vec::new();
    for (name, src, primary, explained) in rows {
        let (_, ds) = checked_with(PRELUDE_SOURCE, src);
        let errors: Vec<_> = (ds.iter())
            .filter(|d| d.code().severity() == Severity::Error)
            .collect();
        let [d] = errors[..] else {
            red.push(format!("row `{name}`: {} errors, wanted 1", errors.len()));
            continue;
        };
        let found = (d.code(), position(src, d.span()));
        if found != (Code::Mismatch, *primary) {
            red.push(format!(
                "row `{name}`: found {found:?}, wanted E0032 at {primary:?}"
            ));
            continue;
        }
        let labels: Vec<_> = (d.secondary().iter())
            .map(|(s, _)| position(src, *s))
            .collect();
        let notes = d.notes().len();
        let rule = d.label().is_some();
        let (wanted, wanted_notes, wanted_rule) = match explained {
            Explained::Label(line, col) => (vec![(*line, *col)], 0, false),
            Explained::Note => (Vec::new(), 1, false),
            Explained::Rule => (Vec::new(), 0, true),
            Explained::Nothing => (Vec::new(), 0, false),
        };
        if labels != wanted || notes != wanted_notes || rule != wanted_rule {
            red.push(format!(
                "row `{name}`: labels at {labels:?}, {notes} notes and a primary label {rule}, \
                 wanted labels at {wanted:?}, {wanted_notes} notes and {wanted_rule}"
            ));
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
fn a_break_origin_should_reach_each_part_of_the_value() {
    origin_rows(&[
        (
            "an element of a `break` value",
            "fn f(c: bool) {\n    let a = loop {\n        if c { break [1] }\n        break [2, \"x\"]\n    }\n    print(a)\n}\nfn main() {\n    f(true)\n}\n",
            (4, 19),
            Explained::Label(3, 16),
        ),
        (
            "a block of a `break` value",
            "fn f(c: bool) {\n    let a = loop {\n        if c { break 1 }\n        break { None }\n    }\n    print(a)\n}\nfn main() {\n    f(true)\n}\n",
            (4, 17),
            Explained::Label(3, 16),
        ),
        (
            "a branch of a `break` value",
            "fn main() {\n    let a = loop {\n        if true { break 1 }\n        break if true { 1 } else { \"x\" }\n    }\n    print(a)\n}\n",
            (4, 36),
            Explained::Label(3, 19),
        ),
        (
            "a branch of a `break` value after a `string` branch",
            "fn f(c: bool, d: bool) {\n    let a = loop {\n        if c { break 1 }\n        break if d { \"x\" } else { 2 }\n    }\n    print(a)\n}\nfn main() {\n    f(true, true)\n}\n",
            (4, 22),
            Explained::Label(3, 16),
        ),
        (
            "a `break` value, the row of S21",
            "fn f(c: bool) {\n    let a = loop {\n        if c { break 1 }\n        break \"x\"\n    }\n    print(a)\n}\nfn main() {\n    f(true)\n}\n",
            (4, 15),
            Explained::Label(3, 16),
        ),
        (
            "a `Some` payload in a `break` value, a form that waits",
            "fn f(c: bool) {\n    let a = loop {\n        if c { break Some(1) }\n        break Some(\"x\")\n    }\n    print(a)\n}\nfn main() { f(true) }\n",
            (4, 20),
            Explained::Nothing,
        ),
        (
            "an `if` in a `break` value with a hint from its siblings",
            "fn f(c: bool, d: bool) {\n    let a = loop {\n        if c { break 1 }\n        break if d { [] } else { \"x\" }\n    }\n    print(a)\n}\nfn main() {\n    f(true, true)\n}\n",
            (4, 22),
            Explained::Label(3, 16),
        ),
    ]);
}

#[test]
fn a_break_origin_should_reach_each_operand_of_the_value() {
    origin_rows(&[
        (
            "the right operand of `+`",
            "fn f(c: bool, d: bool) {\n    let a = loop {\n        if c { break 1 }\n        break 1 + None\n    }\n    print(a)\n}\nfn main() { f(true, false) }\n",
            (4, 19),
            Explained::Label(3, 16),
        ),
        (
            "the left operand of `+`",
            "fn f(c: bool, d: bool) {\n    let a = loop {\n        if c { break 1 }\n        break None + 1\n    }\n    print(a)\n}\nfn main() { f(true, false) }\n",
            (4, 15),
            Explained::Label(3, 16),
        ),
        (
            "a block operand of `+`",
            "fn f(c: bool, d: bool) {\n    let a = loop {\n        if c { break 1 }\n        break 1 + { None }\n    }\n    print(a)\n}\nfn main() { f(true, false) }\n",
            (4, 21),
            Explained::Label(3, 16),
        ),
        (
            "the right operand of `-`",
            "fn f(c: bool, d: bool) {\n    let a = loop {\n        if c { break 1 }\n        break 1 - None\n    }\n    print(a)\n}\nfn main() { f(true, false) }\n",
            (4, 19),
            Explained::Label(3, 16),
        ),
        (
            "the left operand of `*`",
            "fn f(c: bool, d: bool) {\n    let a = loop {\n        if c { break 1 }\n        break None * 1\n    }\n    print(a)\n}\nfn main() { f(true, false) }\n",
            (4, 15),
            Explained::Label(3, 16),
        ),
        (
            "an operand of `+` in an `f64` loop",
            "fn f(c: bool, d: bool) {\n    let a = loop {\n        if c { break 1.0 }\n        break None + 1\n    }\n    print(a)\n}\nfn main() { f(true, false) }\n",
            (4, 15),
            Explained::Label(3, 16),
        ),
        (
            "an operand of a nested `+`",
            "fn f(c: bool) {\n    let a = loop {\n        if c { break 1 }\n        break 1 + (2 + None)\n    }\n    print(a)\n}\nfn main() { f(true) }\n",
            (4, 24),
            Explained::Label(3, 16),
        ),
        (
            "an `f64` leader of `+` in an `i64` loop",
            "fn f(c: bool, x: f64) {\n    let a = loop {\n        if c { break 1 }\n        break x + None\n    }\n    print(a)\n}\nfn main() { f(true, 1.5) }\n",
            (4, 15),
            Explained::Label(3, 16),
        ),
        (
            "an operand of `<`, which the loop does not reach: a comparison has no expected type",
            "fn f(c: bool) {\n    let a = loop {\n        if c { break true }\n        break 1 < None\n    }\n    print(a)\n}\nfn main() { f(true) }\n",
            (4, 19),
            Explained::Nothing,
        ),
        (
            "an operand of `&&`, which takes `bool` whatever the loop is",
            "fn f(c: bool) {\n    let a = loop {\n        if c { break true }\n        break true && None\n    }\n    print(a)\n}\nfn main() { f(true) }\n",
            (4, 23),
            Explained::Nothing,
        ),
        (
            "an operand of `+` held to its own `i64`, not to the `string` of the loop",
            "fn f(c: bool) {\n    let a = loop {\n        if c { break \"a\" }\n        break 1 + None\n    }\n    print(a)\n}\nfn main() { f(true) }\n",
            (4, 19),
            Explained::Nothing,
        ),
        (
            "the fallback of `??`, which takes the payload of its left side",
            "fn f(c: bool, d: bool) {\n    let a = loop {\n        if c { break 1 }\n        break Some(1) ?? \"x\"\n    }\n    print(a)\n}\nfn main() { f(true, false) }\n",
            (4, 26),
            Explained::Nothing,
        ),
        (
            "a `string` leader of `+`, whose join is the origin",
            "fn f(c: bool, s: string) {\n    let a = loop {\n        if c { break 1 }\n        break s + 1\n    }\n    print(a)\n}\nfn main() { f(true, \"s\") }\n",
            (4, 19),
            Explained::Label(4, 15),
        ),
    ]);
}

#[test]
fn a_declaration_should_explain_its_required_type() {
    origin_rows(&[
        (
            "parameter",
            "fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() {\n    print(add(1, \"x\"))\n}\n",
            (3, 18),
            Explained::Label(1, 19),
        ),
        (
            "method",
            "class P {\n    fn m(self, n: i64) -> i64 { n }\n}\nfn f(p: P) {\n    p.m(\"x\")\n}\nfn main() {\n    f(P())\n}\n",
            (5, 9),
            Explained::Label(2, 19),
        ),
        (
            "let",
            "fn main() {\n    let a: i64 = \"x\"\n    print(a)\n}\n",
            (2, 18),
            Explained::Label(2, 12),
        ),
        (
            "return",
            "fn f() -> i64 {\n    return \"x\"\n}\nfn main() {\n    print(f())\n}\n",
            (2, 12),
            Explained::Label(1, 11),
        ),
        (
            "tail",
            "fn f() -> i64 {\n    \"x\"\n}\nfn main() {\n    print(f())\n}\n",
            (2, 5),
            Explained::Label(1, 11),
        ),
        (
            "match tail",
            "enum E {\n    A\n    B\n}\nfn f(e: E) -> i64 {\n    match e {\n        A => 1\n        B => \"x\"\n    }\n}\nfn main() {\n    print(f(E.A))\n}\n",
            (8, 14),
            Explained::Label(5, 15),
        ),
        (
            "field",
            "class P {\n    v: i64\n}\nfn main() {\n    let p = P(v: \"x\")\n    print(p.v)\n}\n",
            (5, 18),
            Explained::Label(2, 8),
        ),
        (
            "payload",
            "enum E {\n    V(i64)\n}\nfn main() {\n    let e = E.V(\"x\")\n    print(e)\n}\n",
            (5, 17),
            Explained::Label(2, 7),
        ),
        (
            "constant",
            "const A: i64 = \"x\"\nfn main() {\n    print(A)\n}\n",
            (1, 16),
            Explained::Label(1, 10),
        ),
        (
            "assignment",
            "fn main() {\n    let mut a = 1\n    a = \"x\"\n    print(a)\n}\n",
            (3, 9),
            Explained::Label(2, 13),
        ),
        (
            "element declared",
            "fn main() {\n    let a: [i64] = [1, \"x\"]\n    print(a)\n}\n",
            (2, 24),
            Explained::Label(2, 12),
        ),
        (
            "a parameter with no type parameter on the generic path (fable-16)",
            "fn f<T>(a: T, n: i64) -> T { a }\nfn main() {\n    print(f(1, \"x\"))\n}\n",
            (3, 16),
            Explained::Label(1, 18),
        ),
        (
            "an assignment to a `mut` parameter (claude-1)",
            "fn f(mut n: i64) {\n    n = \"x\"\n    print(n)\n}\nfn main() {\n    let mut a = 1\n    f(mut a)\n}\n",
            (2, 9),
            Explained::Label(1, 10),
        ),
    ]);
}

#[test]
fn a_prelude_origin_should_be_a_note() {
    origin_rows(&[
        (
            "prelude",
            "fn main() {\n    print(5.min(\"x\"))\n}\n",
            (2, 17),
            Explained::Note,
        ),
        (
            "an array's `get` (opus-35)",
            "fn main() {\n    let a = [1]\n    print(a.get(\"x\"))\n}\n",
            (3, 17),
            Explained::Note,
        ),
    ]);
}

#[test]
fn a_passed_down_expectation_should_keep_its_origin() {
    origin_rows(&[
        (
            "alias let",
            "type M = i64\nfn main() {\n    let a: M = \"x\"\n    print(a)\n}\n",
            (3, 16),
            Explained::Label(3, 12),
        ),
        (
            "an element under an alias (fable-18)",
            "type M = [i64]\nfn main() {\n    let a: M = [1, \"x\"]\n    print(a)\n}\n",
            (3, 20),
            Explained::Label(3, 12),
        ),
        (
            "an operand (fable-1)",
            "fn main() {\n    let a: i64 = 1 + None\n    print(a)\n}\n",
            (2, 22),
            Explained::Label(2, 12),
        ),
        (
            "branch under let",
            "fn main() {\n    let a: i64 = if true { 1 } else { \"x\" }\n    print(a)\n}\n",
            (2, 39),
            Explained::Label(2, 12),
        ),
        (
            "branch under return",
            "fn f() -> i64 {\n    if true { 1 } else { \"x\" }\n}\nfn main() {\n    print(f())\n}\n",
            (2, 26),
            Explained::Label(1, 11),
        ),
        (
            "element of parameter",
            "fn f(a: [i64]) -> i64 { 1 }\nfn main() {\n    print(f([1, \"x\"]))\n}\n",
            (3, 17),
            Explained::Label(1, 9),
        ),
        (
            "explicit type arg, a control: the declaration names a type parameter",
            "fn f<T>(a: T) -> T { a }\nfn main() {\n    let x: i64 = f<i64>(\"x\")\n    print(x)\n}\n",
            (3, 25),
            Explained::Nothing,
        ),
    ]);
}

#[test]
fn an_earlier_value_should_explain_the_type() {
    origin_rows(&[
        (
            "element inferred",
            "fn main() {\n    let a = [1, \"x\"]\n    print(a)\n}\n",
            (2, 17),
            Explained::Label(2, 14),
        ),
        (
            "branch",
            "fn main() {\n    let a = if true { 1 } else { \"x\" }\n    print(a)\n}\n",
            (2, 34),
            Explained::Label(2, 23),
        ),
        (
            "type parameter",
            "fn g<T>(a: T, b: T) -> T { a }\nfn main() {\n    g(1, \"x\")\n}\n",
            (3, 7),
            Explained::Label(3, 10),
        ),
        (
            "an inner `if` against the outer first branch (fable-9)",
            "fn f(c: bool, d: bool) {\n    let a = if c { 1 } else { if d { \"x\" } else { \"y\" } }\n    print(a)\n}\nfn main() {\n    f(true, true)\n}\n",
            (2, 31),
            Explained::Label(2, 20),
        ),
        (
            "an inner join whose first branch gives the type (fable-10)",
            "fn f(c: bool, d: bool) {\n    let a = if c { 1 } else { if d { \"x\" } else { 2 } }\n    print(a)\n}\nfn main() {\n    f(true, true)\n}\n",
            (2, 51),
            Explained::Label(2, 38),
        ),
        (
            "a value that reads the join's hint (fable-20)",
            "fn f(c: bool) {\n    let a = if c { 1 } else { [] }\n    print(a)\n}\nfn main() {\n    f(true)\n}\n",
            (2, 31),
            Explained::Label(2, 20),
        ),
        (
            "an operand under the hint of a branch",
            "fn f(c: bool, n: i64) {\n    let a = if c { n } else { 1 + None }\n    print(a)\n}\nfn main() { f(true, 1) }\n",
            (2, 35),
            Explained::Label(2, 20),
        ),
        (
            "a later arm makes the join optional (opus-41)",
            "fn f(n: i64) {\n    let a = match n {\n        1 => 5\n        2 => Some(1)\n        _ => \"x\"\n    }\n    print(a)\n}\nfn main() {\n    f(1)\n}\n",
            (5, 14),
            Explained::Label(4, 14),
        ),
        (
            "a later element makes the array optional (opus-43)",
            "fn main() {\n    let a = [5, Some(1), \"x\"]\n    print(a)\n}\n",
            (2, 26),
            Explained::Label(2, 17),
        ),
        (
            "the element that set the bound of `T` (opus-12)",
            "fn f<T>(a: [T]) -> i64 { 0 }\nfn g(n: i64) {\n    print(f([None, n, \"x\"]))\n}\nfn main() { g(1) }\n",
            (3, 23),
            Explained::Label(3, 20),
        ),
        (
            "the argument that holds the binding element (opus4-3)",
            "fn has<T>(a: [T], x: T) -> bool { true }\nfn g(n: i64) {\n    print(has([\"a\", \"b\"], n))\n}\nfn main() { g(1) }\n",
            (3, 27),
            Explained::Label(3, 15),
        ),
    ]);
}

/// A branch or an element that took its type from a sibling's hint did not give that type: the
/// label names the value that gave it, in the order of the check.
#[test]
fn a_value_that_took_a_hint_should_not_be_the_origin() {
    origin_rows(&[
        (
            "a literal arm that took a later arm's type (opus-1)",
            "fn f(n: i64, b: u8) {\n    let a = match n {\n        1 => 5\n        2 => b\n        _ => \"x\"\n    }\n    print(a)\n}\nfn main() {}\n",
            (5, 14),
            Explained::Label(4, 14),
        ),
        (
            "an empty array arm that took a later arm's type",
            "fn f(n: i64, b: [i64]) {\n    let a = match n {\n        1 => []\n        2 => b\n        _ => \"x\"\n    }\n    print(a)\n}\nfn main() {}\n",
            (5, 14),
            Explained::Label(4, 14),
        ),
        (
            "a `None` arm before an optional arm (sol-1)",
            "fn f(n: i64, v: i64?) {\n    let a = match n {\n        0 => None\n        1 => v\n        _ => \"x\"\n    }\n    print(a)\n}\nfn main() {}\n",
            (5, 14),
            Explained::Label(4, 14),
        ),
        (
            "a block arm that holds `None`",
            "fn f(n: i64, v: i64?) {\n    let a = match n {\n        0 => { None }\n        1 => v\n        _ => \"x\"\n    }\n    print(a)\n}\nfn main() {}\n",
            (5, 14),
            Explained::Label(4, 14),
        ),
        (
            "an `if` arm whose branches are both `None`",
            "fn f(n: i64, v: i64?, c: bool) {\n    let a = match n {\n        0 => if c { None } else { None }\n        1 => v\n        _ => \"x\"\n    }\n    print(a)\n}\nfn main() {}\n",
            (5, 14),
            Explained::Label(4, 14),
        ),
        (
            "a `None` arm that makes a later arm optional: that arm gives its own type",
            "fn f(n: i64, v: i64) {\n    let a = match n {\n        0 => None\n        1 => v\n        _ => \"x\"\n    }\n    print(a)\n}\nfn main() {}\n",
            (5, 14),
            Explained::Label(4, 14),
        ),
        (
            "a `None` arm before an inner join that gives the type",
            "fn f(n: i64, v: i64?, c: bool) {\n    let a = match n {\n        0 => None\n        1 => if c { v } else { 1 }\n        _ => \"x\"\n    }\n    print(a)\n}\nfn main() {}\n",
            (5, 14),
            Explained::Label(4, 14),
        ),
        (
            "an `else if` chain whose first branch took the type",
            "fn f(c: bool, d: bool, b: u8) {\n    let a = if c { 5 } else if d { b } else { \"x\" }\n    print(a)\n}\nfn main() {}\n",
            (2, 47),
            Explained::Label(2, 36),
        ),
        (
            "a literal element that took a later element's type (opus-2)",
            "fn f(b: u8) {\n    let a = [5, b, \"x\"]\n    print(a)\n}\nfn main() {}\n",
            (2, 20),
            Explained::Label(2, 17),
        ),
        (
            "a literal element that took a later element's type, against a `bool`",
            "fn f(b: u8) {\n    let a = [5, b, true]\n    print(a)\n}\nfn main() {}\n",
            (2, 20),
            Explained::Label(2, 17),
        ),
        (
            "a literal element of a generic call's array that took a later element's type",
            "fn g<T>(a: [T]) -> i64 { 0 }\nfn f(b: u8) {\n    print(g([5, b, \"x\"]))\n}\nfn main() {}\n",
            (3, 20),
            Explained::Label(3, 17),
        ),
        (
            "a `Some` element after `None` and a concrete element",
            "fn f(n: i64) {\n    let a = [None, n, Some(2), \"x\"]\n    print(a)\n}\nfn main() {}\n",
            (2, 32),
            Explained::Label(2, 23),
        ),
    ]);
}

#[test]
fn a_form_that_waits_should_have_no_origin() {
    origin_rows(&[
        (
            "the type that the operator gives in a branch: the branch is checked first (fable-2)",
            "fn f(c: bool) {\n    let a = if c { 1 } else { 2 + None }\n    print(a)\n}\nfn main() {\n    f(true)\n}\n",
            (2, 35),
            Explained::Nothing,
        ),
        (
            "some under let",
            "fn main() {\n    let a: i64? = Some(\"x\")\n    print(a)\n}\n",
            (2, 24),
            Explained::Nothing,
        ),
        (
            "generic prelude",
            "fn main() {\n    let mut a = [1]\n    a.push(\"x\")\n}\n",
            (3, 12),
            Explained::Nothing,
        ),
        (
            "while",
            "fn main() {\n    while 1 { }\n}\n",
            (2, 11),
            Explained::Nothing,
        ),
        (
            "the type that the operator gives (fable-27)",
            "fn main() {\n    print(1 - \"x\")\n}\n",
            (2, 15),
            Explained::Nothing,
        ),
        (
            "a type parameter that the context binds (fable-17)",
            "fn g<T>(a: T, b: T) -> T { a }\nfn main() {\n    let a: i64 = g(\"x\", 2)\n    print(a)\n}\n",
            (3, 20),
            Explained::Nothing,
        ),
        (
            "the fallback of `??` (opus-6)",
            "fn f(o: i64?) {\n    let a: i64 = o ?? \"x\"\n    print(a)\n}\nfn main() {\n    f(None)\n}\n",
            (2, 23),
            Explained::Nothing,
        ),
        (
            "a compound assignment (opus-6)",
            "fn main() {\n    let mut a = 1\n    a += \"x\"\n    print(a)\n}\n",
            (3, 10),
            Explained::Nothing,
        ),
        (
            "an index assignment (opus-6)",
            "fn main() {\n    let mut a = [1]\n    a[0] = \"x\"\n    print(a)\n}\n",
            (3, 12),
            Explained::Nothing,
        ),
        (
            "a match guard (opus-6)",
            "fn f(n: i64) {\n    match n {\n        m if 1 => print(m)\n        _ => print(0)\n    }\n}\nfn main() {\n    f(1)\n}\n",
            (3, 14),
            Explained::Nothing,
        ),
        (
            "a range endpoint (opus-6)",
            "fn main() {\n    for i in 0..\"x\" {\n        print(i)\n    }\n}\n",
            (2, 17),
            Explained::Nothing,
        ),
        (
            "a range endpoint of a range index",
            "fn main() {\n    let a = [1, 2]\n    print(a[0..\"x\"])\n}\n",
            (3, 16),
            Explained::Nothing,
        ),
        (
            "a field assignment (claude-2)",
            "class P {\n    mut v: i64\n}\nfn main() {\n    let p = P(v: 1)\n    p.v = \"x\"\n    print(p.v)\n}\n",
            (6, 11),
            Explained::Nothing,
        ),
    ]);
}

#[test]
fn a_language_rule_should_label_the_primary_span() {
    origin_rows(&[
        (
            "an `if` condition",
            "fn main() {\n    if 1 { print(1) }\n}\n",
            (2, 8),
            Explained::Rule,
        ),
        (
            "a value index",
            "fn main() {\n    let a = [1]\n    print(a[\"x\"])\n}\n",
            (3, 13),
            Explained::Rule,
        ),
        (
            "a value index under a value index",
            "fn main() {\n    let a = [[1]]\n    print(a[0][\"x\"])\n}\n",
            (3, 16),
            Explained::Rule,
        ),
        (
            "a place index",
            "fn main() {\n    let mut a = [1]\n    a[\"x\"] = 2\n    print(a)\n}\n",
            (3, 7),
            Explained::Rule,
        ),
        (
            "a place index under a place index",
            "fn main() {\n    let mut a = [[1]]\n    a[0][\"x\"] = 1\n    print(a)\n}\n",
            (3, 10),
            Explained::Rule,
        ),
        (
            "a place index under `push`",
            "fn main() {\n    let mut a = [[1]]\n    a[\"x\"].push(2)\n    print(a)\n}\n",
            (3, 7),
            Explained::Rule,
        ),
        (
            "a place index under `mut`",
            "fn f(mut v: [i64]) {\n    v = []\n}\nfn main() {\n    let mut a = [[1]]\n    f(mut a[\"x\"])\n    print(a)\n}\n",
            (6, 13),
            Explained::Rule,
        ),
    ]);
}

/// The base of an index that gives no element is reported, and the index after it states no
/// rule: a string or a value that is no array takes no `i64` index either.
#[test]
fn an_index_on_a_base_with_no_element_should_state_no_rule() {
    for (src, base, index) in [
        (
            "fn main() {\n    let s = \"abc\"\n    print(s[\"x\"])\n}\n",
            (3, 11),
            (3, 13),
        ),
        (
            "fn main() {\n    let n = 5\n    print(n[\"x\"])\n}\n",
            (3, 11),
            (3, 13),
        ),
        (
            "fn main() {\n    let mut s = \"abc\"\n    s[\"x\"] = \"y\"\n    print(s)\n}\n",
            (3, 5),
            (3, 7),
        ),
        (
            "fn main() {\n    let mut n = 1\n    n[\"x\"] = 2\n    print(n)\n}\n",
            (3, 5),
            (3, 7),
        ),
    ] {
        codes(src, &[(Code::CannotIndex, base), (Code::Mismatch, index)]);
        let ds = diags(src);
        let mismatch = (ds.iter())
            .find(|d| d.code() == Code::Mismatch)
            .expect("the index is a mismatch");
        assert!(mismatch.label().is_none(), "{src}");
        assert!(mismatch.secondary().is_empty(), "{src}");
    }
}
