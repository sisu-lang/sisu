//! `_` patterns: `let _ = e` and `for _ in ...` bind no name.

use super::*;

#[test]
fn let_underscore_drops_at_once() {
    let printed =
        lowered("fn make() -> [i64] { [1] }\nfn main() {\n    let _ = make()\n    print(1)\n}");
    assert!(printed.contains("(call make) (print 1)"), "{printed}");
    assert!(!printed.contains("(let"), "{printed}");
}

/// A `let` has no value, whatever it lowers to.
#[test]
fn let_underscore_ends_a_block_without_a_value() {
    clean("fn make() -> [i64] { [1] }\nfn main() {\n    let _ = make()\n}");
    clean(
        "fn make() -> [i64] { [1] }\nfn f() -> i64 {\n    let _ = make()\n    1\n}\nfn main() {}",
    );
    codes(
        "fn make() -> [i64] { [1] }\nfn f() -> [i64] {\n    let _ = make()\n}\nfn main() {}",
        &[(Code::BodyHasNoValue, (4, 1))],
    );
}

#[test]
fn let_underscore_checks_its_type_and_value() {
    clean("fn main() {\n    let _: i64 = 1\n}");
    codes(
        "fn main() {\n    let _: bool = 1\n}",
        &[(Code::Mismatch, (2, 19))],
    );
    codes(
        "fn main() {\n    let _ = nope\n}",
        &[(Code::BindingNotFound, (2, 13))],
    );
}

#[test]
fn let_underscore_declares_no_name() {
    codes(
        "fn main() {\n    let _ = 1\n    let _ = 2\n    print(1)\n}",
        &[],
    );
}

#[test]
fn if_let_and_while_let_take_underscore() {
    clean("fn f(n: i64?) {\n    if let _ = n {}\n    while let _ = n {}\n}\nfn main() {}");
    codes(
        "fn main() {\n    if let _ = 5 {}\n}",
        &[(Code::UnwrapNeedsOptional, (2, 16))],
    );
}

/// The declarations the `match` tests share: lines 1 to 12.
const DECLS: &str = "enum Expr {
    Num(i64)
    Add(i64, i64)
}
class C { mut count: i64 }
enum State {
    Idle
    Running(C)
}
enum Shape { Circle(i64) }
enum Opt { V(i64?) }
fn main() {}";

/// `body` on line 14, in a function of line 13 whose parameters are of the shared types.
fn in_fn(body: &str) -> String {
    format!("{DECLS}\nfn t(e: Expr, s: State, p: C, maybe: Expr?, o: Opt, n: i64) {{\n{body}\n}}")
}

/// `body` as a `match` on `scrutinee` whose arm `arm` is on line 15, then `_ => 0`.
fn arm_on_15(scrutinee: &str, arm: &str) -> String {
    in_fn(&format!(
        "    let _ = match {scrutinee} {{\n        {arm}\n        _ => 0\n    }}"
    ))
}

#[test]
fn match_lowers() {
    let rows = [
        (
            "fn f(e: Expr) -> i64 {\n    match e {\n        Num(n) => n\n        Add(a, b) => 0\n    }\n}",
            "(match e#0 (arm (Num n#1) (block n#1)) (arm (Add a#2 b#3) (block 0)))",
        ),
        (
            "fn f(e: Expr) -> i64 {\n    match e {\n        Expr.Num(n) => n\n        Add(_, _) => 1\n    }\n}",
            "(match e#0 (arm (Num n#1) (block n#1)) (arm (Add _ _) (block 1)))",
        ),
        (
            "fn f(s: State) -> i64 {\n    match s {\n        Idle => 0\n        Running(c) => c.count\n    }\n}",
            "(match s#0 (arm Idle (block 0)) (arm (Running c#1) (block (. c#1 count))))",
        ),
        // `if let` on an enum is a `match` of two arms, the second `_`.
        (
            "fn f(s: State) -> i64 {\n    if let Running(c) = s { c.count } else { 0 }\n}",
            "(match s#0 (arm (Running c#1) (block (. c#1 count))) (arm _ (block 0)))",
        ),
        // Without an `else`, the `_` arm is empty.
        (
            "fn f(s: State) {\n    if let Running(c) = s {\n        print(c.count)\n    }\n}",
            "(match s#0 (arm (Running c#1) (block (print (. c#1 count)))) (arm _ (block)))",
        ),
        // `while let` on an enum loops over a `match` whose `_` arm breaks.
        (
            "enum L {\n    Nil\n    Cons(i64, L)\n}\nfn f(start: L) -> i64 {\n    let mut l = start\n    let mut total = 0\n    while let Cons(h, t) = l {\n        total += h\n        l = t\n    }\n    total\n}",
            "(loop (block (match l#1 (arm (Cons h#3 t#4) (block (= total#2 (+ total#2 h#3)) (= l#1 t#4))) (arm _ (block (break))))))",
        ),
    ];
    for (function, part) in rows {
        contains(&format!("{DECLS}\n{function}"), &[part]);
    }
}

/// The type of the local `v` in the function `t` of `src`, which checks clean.
fn type_of_v(src: &str) -> String {
    let (program, ds) = checked(src);
    assert!(ds.is_empty(), "{src}: {ds:?}");
    let program = program.expect("the row should check clean");
    let t = program
        .functions
        .iter()
        .find(|f| f.name == "t")
        .expect("the row should define `t`");
    let v = t
        .locals
        .iter()
        .find(|l| l.name.as_deref() == Some("v"))
        .expect("`t` should bind `v`");
    v.ty.to_string()
}

/// An enum of three variants, declared after a function so the `match` tests' lines stay put:
/// a `_` after `A` and `B` is reachable.
const ABC: &str = "\nenum Abc {\n    A\n    B\n    C\n}";

#[test]
fn match_join() {
    // Each row: the arms' values, in order, and the type they join to.
    let rows: [(&[&str], &str); 10] = [
        (&["1", "return 2", "3"], "i64"),
        // A `never` arm sets no type for the arms after it: the `None` expects `i64?`.
        (&["return 1", "1", "None"], "i64?"),
        (&["1", "b"], "u8"),
        (&["b", "1"], "u8"),
        (&["None", "1"], "i64?"),
        (&["1", "None"], "i64?"),
        (&["[]", "xs"], "[string]"),
        (&["xs", "[]"], "[string]"),
        (&["b", "b", "None"], "u8?"),
        // A block arm ranks by the value it ends in.
        (&["{ None }", "1"], "i64?"),
    ];
    let patterns = ["A", "B", "_"];
    for (values, want) in rows {
        let arms: String = patterns[3 - values.len()..]
            .iter()
            .zip(values)
            .map(|(p, v)| format!("        {p} => {v}\n"))
            .collect::<Vec<_>>()
            .concat();
        let src = format!(
            "{DECLS}\nfn t(e: Abc, b: u8, xs: [string]) -> i64 {{\n    let v = match e {{\n{arms}    }}\n    0\n}}{ABC}"
        );
        assert_eq!(type_of_v(&src), want, "{values:?}");
    }
    // A failed arm sets no type for the arms after it: `b`, checked after it, does, so the `1`
    // expects `u8`.
    codes(
        &format!(
            "{DECLS}\nfn t(e: Abc, b: u8) {{\n    let _ = match e {{\n        A => nope\n        B => b\n        _ => 1\n    }}\n}}{ABC}"
        ),
        &[(Code::BindingNotFound, (15, 14))],
    );
    // A failed arm after one of a known type keeps that type for the arms after it.
    codes(
        &format!(
            "{DECLS}\nfn t(e: Abc, b: u8) {{\n    let _ = match e {{\n        A => b\n        B => nope\n        _ => 1\n    }}\n}}{ABC}"
        ),
        &[(Code::BindingNotFound, (16, 14))],
    );
    // The first arm of a known type anchors the rest, not a later one: the `None` expects `u8?`.
    codes(
        &format!(
            "{DECLS}\nfn t(e: Abc, b: u8) {{\n    let _ = match e {{\n        A => b\n        B => print(1)\n        _ => None\n    }}\n}}{ABC}"
        ),
        &[(Code::NoValue, (16, 14))],
    );
    // A `unit` arm checked first anchors only until an arm of another type: the `None` expects
    // `u8?`.
    codes(
        &format!(
            "{DECLS}\nfn t(e: Abc, b: u8) {{\n    let _ = match e {{\n        A => print(1)\n        B => b\n        _ => None\n    }}\n}}{ABC}"
        ),
        &[(Code::NoValue, (15, 14))],
    );
    // Arms that all leave make a `never` `match`.
    clean(&format!(
        "{DECLS}\nfn t(e: Expr) -> i64 {{\n    match e {{\n        Num(_) => return 1\n        _ => return 2\n    }}\n}}"
    ));
    // The `T` arms of an optional join are wrapped.
    contains(
        &format!(
            "{DECLS}\nfn t(e: Expr) -> i64? {{\n    match e {{\n        Num(_) => None\n        _ => 1\n    }}\n}}"
        ),
        &["(arm _ (block (wrap 1)))"],
    );
    // Arms that do not join: the later one is the mistake, against the earlier.
    codes(
        &arm_on_15("e", "Num(_) => \"a\""),
        &[(Code::Mismatch, (16, 14))],
    );
    // With a type expected, the arm that does not fit it is.
    codes(
        &in_fn("    let _: i64 = match e {\n        Num(_) => \"a\"\n        _ => 0\n    }"),
        &[(Code::Mismatch, (15, 19))],
    );
    // An arm with no value is the mistake.
    codes(
        &arm_on_15("e", "Num(_) => print(1)"),
        &[(Code::NoValue, (15, 19))],
    );
}

#[test]
fn pattern_names() {
    let decls = "const Idle = 0\nconst LIMIT = 9\nconst limit = 9";
    // Each row: the scrutinee, the arm on line 18, and the code it reports at column 9.
    let rows = [
        ("s", "Stopped => 1", Some(Code::UnknownVariant)),
        ("n", "Stopped => 1", Some(Code::NotVariantHere)),
        // The name still binds, so the body reports nothing more.
        ("n", "Count => Count + 1", Some(Code::NotVariantHere)),
        // That binding is not reported again for taking an enum's name.
        ("n", "State => 1", Some(Code::NotVariantHere)),
        ("n", "LIMIT => 1", Some(Code::ConstantInPattern)),
        ("n", "limit => 1", Some(Code::ConstantInPattern)),
        // A variant of the enum comes before the constant of its name.
        ("s", "Idle => 1", None),
        ("s", "State.Idle => 1", None),
    ];
    for (scrutinee, arm, code) in rows {
        let src = format!("{decls}\n{}", arm_on_15(scrutinee, arm));
        let expected: Vec<_> = code.into_iter().map(|c| (c, (18, 9))).collect();
        codes(&src, &expected);
    }
    contains(
        &format!("{decls}\n{}", arm_on_15("s", "Idle => 1")),
        &["(arm Idle (block 1))"],
    );
}

#[test]
fn a_bad_pattern_should_report_its_code_at_its_position() {
    // Each row: the scrutinee, the arm on line 15, and the code it reports there.
    let rows = [
        ("e", "Shape.Circle(r) => 1", Code::Mismatch, 9),
        ("e", "Num(Num(m)) => 1", Code::Mismatch, 13),
        ("n", "Num(m) => 1", Code::Mismatch, 9),
        ("n", "None => 1", Code::Mismatch, 9),
        ("e", "Add(x) => 1", Code::SubpatternCount, 9),
        ("e", "Add(x, x) => 1", Code::BoundTwice, 16),
        ("s", "Running(Idle) => 1", Code::NotVariantHere, 17),
        ("s", "Running(Num(m)) => 1", Code::OnlyNamePosition, 17),
        // A variant pattern at an optional position matches the value inside.
        ("o", "V(Num(m)) => 1", Code::Mismatch, 11),
        // `Some` at an optional position needs its one payload.
        ("maybe", "Some => 1", Code::VariantForm, 9),
        ("maybe", "Option.Some => 1", Code::VariantForm, 16),
        ("maybe", "Some(x, y) => 1", Code::SubpatternCount, 9),
        // `Option.None` holds nothing.
        ("maybe", "Option.None() => 1", Code::VariantForm, 20),
        ("maybe", "Option.Sone(x) => 1", Code::UnknownVariant, 16),
        ("s", "Idle() => 1", Code::VariantForm, 13),
        ("e", "Expr.Num => 1", Code::VariantForm, 14),
        ("e", "Num => 1", Code::VariantForm, 9),
        ("e", "Nmu(m) => 1", Code::UnknownVariant, 9),
        ("e", "Exr.Num(m) => 1", Code::UnknownType, 9),
        ("e", "Num(x, y) => 1", Code::SubpatternCount, 9),
        // A failed pattern binds each name once, so `x` is not bound twice.
        ("e", "Mul(x, x) => 1", Code::UnknownVariant, 9),
        // An `|` in a failed pattern binds its names too.
        ("e", "Mul(Num(x) | Add(x, _)) => x", Code::UnknownVariant, 9),
        // A body fails on its own, after a pattern that checks.
        ("e", "Num(m) => nope", Code::BindingNotFound, 19),
        // The fields after a failed one are checked too, so their names are bound.
        ("e", "Add(Nmu, b) => b", Code::NotVariantHere, 13),
        ("e", "Add(Num(x, y), m) => m", Code::Mismatch, 13),
        ("e", "Add(None, y) => y", Code::Mismatch, 13),
    ];
    for (scrutinee, arm, code, col) in rows {
        codes(&arm_on_15(scrutinee, arm), &[(code, (15, col))]);
    }
    // A built-in name still binds, so it takes every value and the `_` after it is unreachable.
    codes(
        &arm_on_15("n", "Ok => 1"),
        &[
            (Code::BuiltInName, (15, 9)),
            (Code::UnreachableArm, (16, 9)),
        ],
    );
    // Each row: the statement on line 14, and the code it reports there.
    let rows = [
        ("    let _ = match p {}", Code::CannotMatch, 19),
        ("    if let m = 5 {}", Code::UnwrapNeedsOptional, 16),
        ("    while let Num(m) = 5 {}", Code::UnwrapNeedsOptional, 24),
        ("    if let m = p {}", Code::UnwrapNeedsOptional, 16),
        ("    while let Nmu(m) = e {}", Code::UnknownVariant, 15),
        (
            "    while let Add(Q, y) = e { print(y) }",
            Code::NotVariantHere,
            19,
        ),
    ];
    for (stmt, code, col) in rows {
        codes(&in_fn(stmt), &[(code, (14, col))]);
    }
    // A name bound twice that is no variant is reported each time, as no variant only.
    codes(
        &arm_on_15("e", "Add(Q, Q) => 1"),
        &[
            (Code::NotVariantHere, (15, 13)),
            (Code::NotVariantHere, (15, 16)),
        ],
    );
    // A payload of an unknown type matches any sub-pattern, and binds its names as errors.
    codes(
        &format!(
            "{DECLS}\nenum W {{ Wrap(Nope) }}\nfn f(w: W) -> i64 {{\n    match w {{\n        Wrap(Num(m)) => m + 1\n    }}\n}}"
        ),
        &[(Code::UnknownType, (13, 15))],
    );
    // The names of a failed pattern are bound as errors, so the body reports nothing more.
    codes(
        &arm_on_15("e", "Add(x) => x + 1"),
        &[(Code::SubpatternCount, (15, 9))],
    );
}

/// A pattern of another enum is reported from its qualifier on; the fixtures pin the other
/// pattern spans.
#[test]
fn pattern_spans() {
    diagnostic(
        &arm_on_15("e", "Shape.Circle(r) => 1"),
        Code::Mismatch,
        (15, 9, "Shape.Circle(r)"),
        &[],
    );
}

#[test]
fn variant_form_fix_is_safe_only_when_empty() {
    let src = arm_on_15("s", "Idle() => 1");
    let d = diagnostic(&src, Code::VariantForm, (15, 13, "()"), &[]);
    assert_eq!(
        fixes(&src, &d),
        [(true, vec![(15, 13, "()", String::new())])]
    );
    fix_removes_its_code(&src, &d);
    let src = arm_on_15("s", "Idle(// keep\n        ) => 1");
    let text = "(// keep\n        )";
    let d = diagnostic(&src, Code::VariantForm, (15, 13, text), &[]);
    assert_eq!(
        fixes(&src, &d),
        [(false, vec![(15, 13, text, String::new())])]
    );
}

/// The fix deletes the parentheses of `Option.None()`, so a name or a comment in them makes it
/// unsafe.
#[test]
fn option_none_parentheses_should_have_a_safe_fix_only_when_empty() {
    for (written, text, safe) in [
        ("()", "()", true),
        ("(x)", "(x)", false),
        ("(// keep\n        )", "(// keep\n        )", false),
    ] {
        let src = arm_on_15("maybe", &format!("Option.None{written} => 1"));
        let d = diagnostic(&src, Code::VariantForm, (15, 20, text), &[]);
        assert_eq!(
            fixes(&src, &d),
            [(safe, vec![(15, 20, text, String::new())])],
            "{written}"
        );
    }
}

#[test]
fn pattern_bindings_are_immutable() {
    // The secondary span is the binding, which the label says `match` bound.
    diagnostic(
        &format!(
            "{DECLS}\nfn t(e: Expr) {{\n    match e {{\n        Add(w, h) => w = 5\n        _ => {{}}\n    }}\n}}"
        ),
        Code::AssignToImmutable,
        (15, 22, "w = 5"),
        &[(15, 13)],
    );
}

#[test]
fn match_spills() {
    // A counted argument before a `match` that exits is spilled, as before an `if`.
    let src = format!(
        "{DECLS}\nfn g(xs: [i64], k: i64) -> i64 {{ k }}\nfn f(xs: [i64], e: Expr) -> i64 {{\n    let _ = g(xs, match e {{\n        Num(_) => return 1\n        _ => 2\n    }})\n    0\n}}"
    );
    contains(
        &src,
        &[
            "(let #3 xs#0) (let #4 (match e#1 (arm (Num _) (block (return 1))) (arm _ (block 2)))) (call g #3 #4)",
        ],
    );
}

/// `i64(...)` around a `match` that is already an `i64` keeps the `match` grouped unless
/// parentheses already group it.
#[test]
fn already_converted_match_stays_grouped() {
    let src = in_fn("    let _ = i64(match e {\n        _ => 1\n    }) + 1");
    let d = diagnostic(
        &src,
        Code::AlreadyConverted,
        (14, 17, "match e {\n        _ => 1\n    }"),
        &[],
    );
    let fixed = applied(&src, &d);
    assert!(fixed.contains("let _ = (match e {"), "{fixed}");
    let src = in_fn("    let _ = i64((match e {\n        _ => 1\n    })) + 1");
    let d = diagnostic(
        &src,
        Code::AlreadyConverted,
        (14, 17, "(match e {\n        _ => 1\n    })"),
        &[],
    );
    let fixed = applied(&src, &d);
    assert!(fixed.contains("let _ = (match e {"), "{fixed}");
}

/// A scrutinee `match` cannot take fails the `match` as a failed condition fails an `if`: its
/// arms' `never` stays, and any other type becomes `Error`. With no arms, a scrutinee of a known
/// type makes it `never`, and an unknown one `Error`.
#[test]
fn failed_scrutinee_keeps_never() {
    codes(
        &format!(
            "{DECLS}\nfn f(p: C) -> i64 {{\n    match p {{\n        _ => return 1\n    }}\n    print(3)\n}}"
        ),
        &[
            (Code::CannotMatch, (14, 11)),
            (Code::UnreachableCode, (17, 5)),
        ],
    );
    codes(
        &in_fn("    let x = match p {\n        _ => 1\n    }\n    let _: string = x"),
        &[(Code::CannotMatch, (14, 19))],
    );
    codes(
        &in_fn("    match n {}\n    print(3)"),
        &[
            (Code::NonExhaustive, (14, 5)),
            (Code::UnreachableCode, (15, 5)),
        ],
    );
    codes(
        &in_fn("    match nope {}\n    print(3)"),
        &[(Code::BindingNotFound, (14, 11))],
    );
}

/// The declarations the coverage tests share: lines 1 to 10.
const COVER: &str = "enum Expr {
    Num(i64)
    Add(Expr, Expr)
    Mul(Expr, Expr)
}
enum AB {
    A
    B
}
fn main() {}";

/// A function on line 11 whose `match` on `scrutinee` has its keyword at (12, 5), each of
/// `arms` on its own line from line 13, and its `}` on the line after them.
fn match_on(scrutinee: &str, arms: &[&str]) -> String {
    let arms = arms
        .iter()
        .map(|arm| format!("        {arm}\n"))
        .collect::<Vec<_>>()
        .concat();
    format!(
        "{COVER}\nfn t(e: Expr, ab: AB, i: i64, flag: bool) -> i64 {{\n    match {scrutinee} {{\n{arms}    }}\n}}"
    )
}

#[test]
fn match_coverage() {
    let rows: [(&str, &[&str], bool); 4] = [
        ("e", &["Num(_) => 0", "Add(_, _) => 1"], false),
        (
            "e",
            &["Num(_) => 0", "Add(Num(_), _) => 1", "Mul(_, _) => 2"],
            false,
        ),
        ("i", &[], false),
        ("flag", &["b => 0"], true),
    ];
    for (scrutinee, arms, covered) in rows {
        let expected = if covered {
            vec![]
        } else {
            vec![(Code::NonExhaustive, (12, 5))]
        };
        codes(&match_on(scrutinee, arms), &expected);
    }
    // A column of an enum with no variant goes to the default, as a `_`.
    codes(
        "enum E {}\nenum Bit {\n    X\n    Y\n}\nenum F { G(E, Bit) }\nfn t(f: F) -> i64 {\n    match f {\n        G(_, X) => 1\n    }\n}\nfn main() {}",
        &[(Code::EmptyEnum, (1, 6)), (Code::NonExhaustive, (8, 5))],
    );
}

/// The note names the first arm that has the missing value's variant at the top, and the fix
/// adds `_ => todo` as the last arm: on the `}` line's own line, or by breaking `{}`.
#[test]
fn non_exhaustive_note_and_fix() {
    let src = match_on(
        "e",
        &["Num(_) => 0", "Add(Num(_), _) => 1", "Mul(_, _) => 2"],
    );
    let d = diagnostic(&src, Code::NonExhaustive, (12, 5, "match"), &[(14, 9)]);
    assert_eq!(
        fixes(&src, &d),
        [(false, vec![(16, 1, "", "        _ => todo\n".to_string())])]
    );
    applied(&src, &d);
    let src = format!("{COVER}\nfn t(i: i64) -> i64 {{\n    match i {{}}\n}}");
    let d = diagnostic(&src, Code::NonExhaustive, (12, 5, "match"), &[]);
    assert_eq!(
        fixes(&src, &d),
        [(
            false,
            vec![(12, 14, "", "\n        _ => todo\n    ".to_string())]
        )]
    );
    applied(&src, &d);
    // Arms on the `match` line take one level past the line's indentation, not past `let`.
    let src =
        format!("{COVER}\nfn t(ab: AB) -> i64 {{\n    let v = match ab {{ A => 1 }}\n    v\n}}");
    let d = diagnostic(&src, Code::NonExhaustive, (12, 13, "match"), &[]);
    // The fix starts at the last arm's end, so no space trails it.
    assert_eq!(
        fixes(&src, &d),
        [(
            false,
            vec![(12, 30, " ", "\n        _ => todo\n    ".to_string())]
        )]
    );
    applied(&src, &d);
    // The `match`'s own `}` places the fix, not a `}` in a comment after it.
    let src = format!(
        "{COVER}\nfn t(ab: AB) -> i64 {{\n    let v = (match ab {{\n        A => 1\n    }} // }}\n    )\n    v\n}}"
    );
    let d = diagnostic(&src, Code::NonExhaustive, (12, 14, "match"), &[]);
    assert_eq!(
        fixes(&src, &d),
        [(false, vec![(14, 1, "", "        _ => todo\n".to_string())])]
    );
    applied(&src, &d);
}

#[test]
fn unreachable_arms() {
    let src = match_on("e", &["_ => 0", "Num(n) => n"]);
    let d = diagnostic(&src, Code::UnreachableArm, (14, 9, "Num(n)"), &[(13, 9)]);
    // The fix takes the arm's whole line.
    assert_eq!(
        fixes(&src, &d),
        [(false, vec![(14, 1, "        Num(n) => n\n", String::new())])]
    );
    applied(&src, &d);
    // A comment after the arm goes with it.
    let src = match_on("ab", &["A => 1", "B => 2", "A => 3 // why"]);
    let d = diagnostic(&src, Code::UnreachableArm, (15, 9, "A"), &[(13, 9)]);
    assert_eq!(
        fixes(&src, &d),
        [(
            false,
            vec![(15, 1, "        A => 3 // why\n", String::new())]
        )]
    );
    applied(&src, &d);
    // With the `match`'s `}` on the arm's line, the fix takes the line break before the arm
    // through its body, and the `}` stays.
    let src = format!(
        "{COVER}\nfn t(ab: AB) -> i64 {{\n    match ab {{\n        A => 1\n        B => 2\n        A => 3 }}\n}}"
    );
    let d = diagnostic(&src, Code::UnreachableArm, (15, 9, "A"), &[(13, 9)]);
    assert_eq!(
        fixes(&src, &d),
        [(false, vec![(14, 15, "\n        A => 3", String::new())])]
    );
    applied(&src, &d);
    diagnostic(
        &match_on("e", &["Num(n) => n", "Num(_) => 0", "_ => 1"]),
        Code::UnreachableArm,
        (14, 9, "Num(_)"),
        &[(13, 9)],
    );
    // Three arms together cover the fourth, so no one arm does.
    diagnostic(
        &match_on(
            "e",
            &[
                "Add(Num(_), _) => 0",
                "Add(Mul(_, _), _) => 1",
                "Add(Add(_, _), _) => 2",
                "Add(_, _) => 3",
                "_ => 4",
            ],
        ),
        Code::UnreachableArm,
        (16, 9, "Add(_, _)"),
        &[],
    );
    // The first arm takes the second in two leaves, split apart by the third, and still covers it.
    diagnostic(
        &match_on(
            "e",
            &[
                "Add(_, Num(_)) => 0",
                "Add(_, Num(_)) => 1",
                "Add(Num(_), _) => 2",
                "_ => 3",
            ],
        ),
        Code::UnreachableArm,
        (14, 9, "Add(_, Num(_))"),
        &[(13, 9)],
    );
    // Each arm is covered by the arm of its own variant, not by the first arm.
    let src = match_on("ab", &["A => 0", "A => 1", "B => 2", "B => 3"]);
    assert_eq!(
        found(&src),
        [
            (Code::UnreachableArm, (14, 9), vec![(13, 9)]),
            (Code::UnreachableArm, (16, 9), vec![(15, 9)]),
        ]
    );
}

/// The solver's steps to check `src`, whose diagnostics are exactly `expected`.
fn coverage_steps(src: &str, expected: &[(Code, (usize, usize))]) -> usize {
    exhaust::STEPS.set(0);
    codes(src, expected);
    exhaust::STEPS.get()
}

/// A pattern of the typed tree. The span is where the row sits: `at` through `at + 1`.
fn row_pattern(at: usize, kind: tir::PatternKind) -> tir::Pattern {
    tir::Pattern {
        kind,
        span: Span::new(at, at + 1),
    }
}

/// A pattern inside a row.
fn field_pattern(kind: tir::PatternKind) -> tir::Pattern {
    row_pattern(0, kind)
}

/// The enum with one variant for each entry of `variants`, the types of that variant's fields.
fn enum_of(variants: Vec<Vec<Type>>) -> tir::Enum {
    let variants = (variants.into_iter().enumerate())
        .map(|(i, fields)| tir::Variant {
            name: format!("V{i}"),
            fields: (fields.into_iter())
                .map(|ty| tir::Payload {
                    ty,
                    label: None,
                    boxed: false,
                    rank: tir::Rank::Plain,
                })
                .collect(),
        })
        .collect();
    tir::Enum {
        name: String::from("E"),
        type_params: Vec::new(),
        variants,
        counted: false,
        cycle: None,
    }
}

/// The type of the enum at index `id` of the program's enums.
fn enum_type(id: usize) -> Type {
    Type::intern(TypeKind::Enum(tir::EnumId(id), TypeArgs::NONE))
}

/// `exhaust::check` on `patterns`, one unguarded row each: the rows no value reaches, as the
/// spans of their patterns; `enums` are the program's and `ty` is the type of the `match`.
/// The report has no missing value.
fn unreachable_rows(enums: &[tir::Enum], ty: Type, patterns: &[tir::Pattern]) -> Vec<Span> {
    let rows: Vec<_> = patterns
        .iter()
        .map(|pattern| exhaust::Row {
            pattern,
            guarded: false,
        })
        .collect();
    let report = exhaust::check(enums, &tir::Conformances::default(), ty, &rows);
    assert!(
        report.missing.is_none(),
        "the rows should cover every value"
    );
    assert!(
        report.unreachable.iter().all(|u| u.alternative.is_none()),
        "every alternative should be reached"
    );
    report.unreachable.iter().map(|u| u.at).collect()
}

/// The kill override for this test is in `.config/nextest.toml`: the rows of a `match` on an enum
/// of 145,000 variants, with an arm for each, given to `exhaust::check`. Finding a variant's
/// bucket by a search of the buckets, or specializing the column once per variant over every
/// row, is quadratic and hangs. A counter cannot see these: the search is a new loop, not a step
/// the solver counts. On a developer machine at load 1 the current code takes 0.2 s, the search
/// of the buckets 24 s, and the scan of every row for each bucket more than 300 s; the kill is
/// 4 s.
#[test]
fn variants_should_check_in_linear_time() {
    const VARIANTS: usize = 145_000;
    let enums = [enum_of(vec![Vec::new(); VARIANTS])];
    let rows: Vec<_> = (0..VARIANTS)
        .map(|i| {
            row_pattern(
                i,
                tir::PatternKind::Variant {
                    variant: i,
                    fields: Vec::new(),
                },
            )
        })
        .collect();
    assert_eq!(unreachable_rows(&enums, enum_type(0), &rows), []);
}

/// The kill override for this test is in `.config/nextest.toml`: the rows of a `match` on 140,000
/// literals, given to `exhaust::check`. A split that rescans the column for each row, or finds a
/// row's first interval by a search from the start, is quadratic in the literals and hangs. A
/// counter cannot see these, as for `variants_should_check_in_linear_time`. On a developer
/// machine at load 1 the current code takes 0.3 s, the search from the start 20 s, the search of
/// the buckets 26 s and the rescan of the whole column 80 s; the kill is 4 s.
#[test]
fn literal_arms_should_check_in_linear_time() {
    const LITERALS: usize = 140_000;
    let literal = |i: usize| tir::PatternKind::Range {
        lo: i64::try_from(i).expect("a literal should fit an `i64`"),
        hi: i64::try_from(i).expect("a literal should fit an `i64`"),
    };
    let rows: Vec<_> = (0..LITERALS)
        .map(|i| row_pattern(i, literal(i)))
        .chain([row_pattern(LITERALS, tir::PatternKind::Wild)])
        .collect();
    assert_eq!(unreachable_rows(&[], Type::I64, &rows), []);
}

/// `V(true | false, ...)` of `n` fields. Specializing a field copies the rest of its row, so a
/// row of `n` fields costs `n` times `n`. `push` counts each stack node that such a copy builds.
/// A walk of the rest of the row builds no node: `long_or_row_should_check_in_linear_time` guards
/// that. Every field is an `|`, so no row is all wildcards and the cut cannot skip a column.
#[test]
fn or_fields_should_check_in_linear_time() {
    let steps = |n: usize| {
        let fields = vec!["bool"; n].join(", ");
        let pattern = vec!["true | false"; n].join(", ");
        coverage_steps(
            &format!(
                "enum F {{ V({fields}) }}\nfn f(x: F) -> i64 {{\n    match x {{\n        V({pattern}) => 1\n    }}\n}}\nfn main() {{}}"
            ),
            &[],
        )
    };
    assert_near_linear(steps(100), steps(200), steps(300));
}

/// The kill override for this test is in `.config/nextest.toml`: one row of 60,000 `true | false`
/// fields, given to `exhaust::check`. A `push` that walks the whole rest of the row, to rehash it
/// or to read its flags, is quadratic in the fields and hangs. A counter cannot see this,
/// because the walk builds no stack node. Each `|` has a span of its own, as in a source. On a
/// developer machine at load 1 the current code takes 0.3 s and the walk 17 s; the kill is 4 s.
#[test]
fn long_or_row_should_check_in_linear_time() {
    const FIELDS: usize = 60_000;
    let enums = [enum_of(vec![vec![Type::BOOL; FIELDS]])];
    // As in a source, each `|` and each alternative has a span of its own, so none is the same
    // pattern as another.
    let either = |k: usize| {
        row_pattern(
            3 * k,
            tir::PatternKind::Or(vec![
                row_pattern(3 * k + 1, tir::PatternKind::Bool(true)),
                row_pattern(3 * k + 2, tir::PatternKind::Bool(false)),
            ]),
        )
    };
    let row = row_pattern(
        0,
        tir::PatternKind::Variant {
            variant: 0,
            fields: (0..FIELDS).map(either).collect(),
        },
    );
    assert_eq!(unreachable_rows(&enums, enum_type(0), &[row]), []);
}

/// The kill override for this test is in `.config/nextest.toml`: the rows of a `match` on
/// `Q(E, i64)`, given to `exhaust::check`. The first arm has 2,400 alternatives of `E`. Then come
/// 1,000 literal arms and 1,000 range arms of the `i64`, and each range arm is unreachable. The
/// arms below the first are in the bucket of each alternative, and the literal arms take the
/// range arms. A list of every row that took a row's values costs alternatives times literals
/// times ranges and hangs. A counter cannot see this, because the list is a new loop. On a
/// developer machine at load 2 the current code takes 0.5 s and the list 12.5 s; the kill is
/// 4 s.
#[test]
fn takers_should_check_in_linear_time() {
    const ALTERNATIVES: usize = 2_400;
    const TAKERS: usize = 1_000;
    let enums = [
        enum_of(vec![Vec::new(); ALTERNATIVES]),
        enum_of(vec![vec![enum_type(0), Type::I64]]),
    ];
    let range = |lo: i64, hi: i64| field_pattern(tir::PatternKind::Range { lo, hi });
    let wild = || field_pattern(tir::PatternKind::Wild);
    let alternatives = (0..ALTERNATIVES)
        .map(|variant| {
            field_pattern(tir::PatternKind::Variant {
                variant,
                fields: Vec::new(),
            })
        })
        .collect();
    let q = |at: usize, fields: Vec<tir::Pattern>| {
        row_pattern(at, tir::PatternKind::Variant { variant: 0, fields })
    };
    let last_literal = i64::try_from(TAKERS).expect("the takers should fit an `i64`") - 1;
    let mut rows = vec![q(
        0,
        vec![
            field_pattern(tir::PatternKind::Or(alternatives)),
            range(-1, -1),
        ],
    )];
    for j in 0..=last_literal {
        rows.push(q(rows.len(), vec![wild(), range(j, j)]));
    }
    let ranges = rows.len()..rows.len() + TAKERS;
    for _ in 0..TAKERS {
        rows.push(q(rows.len(), vec![wild(), range(0, last_literal)]));
    }
    rows.push(q(rows.len(), vec![wild(), wild()]));
    let expected: Vec<_> = ranges.map(|at| Span::new(at, at + 1)).collect();
    assert_eq!(unreachable_rows(&enums, enum_type(1), &rows), expected);
}

/// The kill override for this test is in `.config/nextest.toml`: a source with an enum of 44,000
/// variants, a `match` with an arm for each, and a `match` whose first arm is one `|` of 4,400 of
/// them. The solver tests above build their rows by hand, so this one gives the parser and the
/// lowering of names and patterns (`check/pattern.rs`) a large well-formed `match`. A search of
/// the variants for a name, in place of the index, is quadratic and hangs. On a developer machine
/// the current code takes 0.35 s, and the search 9.9 s at load 13; the kill is 4 s.
#[test]
fn match_on_many_variants_should_check_in_linear_time() {
    const VARIANTS: usize = 44_000;
    const ALTERNATIVES: usize = 4_400;
    let variants = (0..VARIANTS)
        .map(|i| format!("    V{i}\n"))
        .collect::<Vec<_>>()
        .concat();
    let arms = (0..VARIANTS)
        .map(|i| format!("        V{i} => {i}\n"))
        .collect::<Vec<_>>()
        .concat();
    let names = (0..ALTERNATIVES)
        .map(|i| format!("V{i}"))
        .collect::<Vec<_>>()
        .join(" | ");
    clean(&format!(
        "enum E {{\n{variants}}}\nfn f(e: E) -> i64 {{\n    match e {{\n{arms}    }}\n}}\nfn g(e: E) -> i64 {{\n    match e {{\n        {names} => 1\n        _ => 0\n    }}\n}}\nfn main() {{}}"
    ));
}

/// The keep-alive override for this test is in `.config/nextest.toml`: finding the arm that
/// covers each unreachable alternative by solving again the arms above it is quadratic. So is
/// copying every alternative's takers into each matrix above it, which a payload 20,000 fields
/// deep with an `|` in each makes 20,000 matrices deep, or recording the alternatives of a row
/// that each of 8,000 arms' buckets cuts, 8,000 of them, once per bucket. This test then hangs
/// instead of failing. The duplicate whole arms took their covering arm from the one solve
/// before, and guard that it stays so.
#[test]
fn many_unreachable_rows_check_in_linear_time() {
    const VARIANTS: usize = 20_000;
    const FIELDS: usize = 20_000;
    const CUTS: usize = 8_000;
    let variants = (0..CUTS)
        .map(|i| format!("    V{i}\n"))
        .collect::<Vec<_>>()
        .concat();
    let arms = (0..CUTS)
        .map(|i| format!("        Q(V{i}, _) => 0\n"))
        .collect::<Vec<_>>()
        .concat();
    let fields = vec!["F"; CUTS].join(", ");
    let pattern = vec!["A | B"; CUTS].join(", ");
    clean(&format!(
        "enum E {{\n{variants}    Extra\n}}\nenum F {{\n    A\n    B\n}}\nenum W {{ X({fields}) }}\nenum P {{ Q(E, W) }}\nfn f(p: P) -> i64 {{\n    match p {{\n{arms}        Q(_, X({pattern})) => 1\n    }}\n}}\nfn main() {{}}"
    ));
    let fields = vec!["bool"; FIELDS].join(", ");
    let pattern = vec!["true | false"; FIELDS].join(", ");
    let (src, line) = typed(
        &format!("enum F {{ V({fields}) }}\n"),
        "F",
        &[&format!("V({pattern}) => 0"), &format!("V({pattern}) => 1")],
    );
    assert_eq!(
        found(&src),
        [(Code::UnreachableArm, (line + 2, 9), vec![(line + 1, 9)])]
    );
    let variants = (0..VARIANTS)
        .map(|i| format!("    V{i}\n"))
        .collect::<Vec<_>>()
        .concat();
    for arm in ["V{i} => 0\n        V{i} => 1", "V{i} | V{i} => 0"] {
        let arms = (0..VARIANTS)
            .map(|i| format!("        {}\n", arm.replace("{i}", &i.to_string())))
            .collect::<Vec<_>>()
            .concat();
        let src = format!(
            "enum E {{\n{variants}}}\nfn f(e: E) -> i64 {{\n    match e {{\n{arms}    }}\n}}\nfn main() {{}}"
        );
        let found = diags(&src);
        assert_eq!(found.len(), VARIANTS, "{arm}");
        // A second arm is covered by its variant's first arm; a second alternative by no arm,
        // since its own arm's first alternative takes it.
        let twins = arm.contains("=> 1");
        for d in &found {
            assert_eq!(d.code(), Code::UnreachableArm, "{arm}");
            let covering: Vec<_> = d.secondary().iter().map(|(s, _)| &src[s.range()]).collect();
            let expected = if twins {
                vec![&src[d.span().range()]]
            } else {
                vec![]
            };
            assert_eq!(covering, expected, "{arm}");
        }
    }
}

/// The keep-alive override for this test is in `.config/nextest.toml`: a variant pattern that
/// copies its variant's field types before it checks its form costs the variant's width, so short
/// malformed patterns of a wide variant are quadratic and this test hangs instead of failing.
#[test]
fn malformed_patterns_of_a_wide_variant_check_in_linear_time() {
    const FIELDS: usize = 100_000;
    let fields = vec!["i64"; FIELDS].join(", ");
    let arms = "        E.V => 0\n".repeat(FIELDS);
    let src = format!(
        "enum E {{ V({fields}) }}\nfn f(e: E) -> i64 {{\n    match e {{\n{arms}    }}\n}}\nfn main() {{}}"
    );
    let codes: Vec<_> = diags(&src).iter().map(Diagnostic::code).collect();
    assert_eq!(codes, vec![Code::VariantForm; FIELDS]);
}

/// A payload of 32,768 fields gives a matrix 32,768 columns deep, which the solver must not meet
/// with as deep a recursion.
#[test]
fn wide_payload_checks_without_deep_recursion() {
    const FIELDS: usize = 32_768;
    let arm = wide_arm(FIELDS, |f| if f == FIELDS - 1 { "X" } else { "_" });
    clean(&wide(FIELDS, &[arm]));
}

/// A witness copied at each of its columns costs the columns times their number.
#[test]
fn wide_witness_should_build_in_linear_time() {
    let steps = |n: usize| {
        let fields = vec!["Bit"; n].join(", ");
        let arm = wide_arm(n, |_| "X");
        coverage_steps(
            &format!(
                "enum Bit {{\n    X\n    Y\n}}\nenum Wide {{ V({fields}) }}\nfn t(w: Wide) -> i64 {{\n    match w {{\n        {arm}\n    }}\n}}\nfn main() {{}}"
            ),
            &[(Code::NonExhaustive, (7, 5))],
        )
    };
    assert_near_linear(steps(100), steps(200), steps(300));
}

/// Each `Wide` column below the root has a bucket for `W` and a default that stands for `V`, so a
/// default's head built anew at each of them costs `n` columns times `n` fields.
#[test]
fn default_witness_head_should_build_in_linear_time() {
    let steps = |n: usize| {
        let bits = vec!["Bit"; n].join(", ");
        let wides = vec!["Wide"; n].join(", ");
        let ws = vec!["W"; n].join(", ");
        coverage_steps(
            &format!(
                "enum Bit {{\n    X\n    Y\n}}\nenum Wide {{\n    V({bits})\n    W\n}}\nenum Row {{ P({wides}) }}\nfn t(r: Row) -> i64 {{\n    match r {{\n        P({ws}) => 1\n    }}\n}}\nfn main() {{}}"
            ),
            &[(Code::NonExhaustive, (11, 5))],
        )
    };
    assert_near_linear(steps(100), steps(200), steps(300));
}

/// `enum Wide { V(Bit, ...) }` with `fields` fields, and a `match` on `w` whose `arms` start on
/// line 8, then `_ => 0`.
fn wide(fields: usize, arms: &[String]) -> String {
    let fields = vec!["Bit"; fields].join(", ");
    let arms = arms
        .iter()
        .map(|arm| format!("        {arm}\n"))
        .collect::<Vec<_>>()
        .concat();
    format!(
        "enum Bit {{\n    X\n    Y\n}}\nenum Wide {{ V({fields}) }}\nfn f(w: Wide) -> i64 {{\n    match w {{\n{arms}        _ => 0\n    }}\n}}\nfn main() {{}}"
    )
}

/// `V(...)` of `count` fields, field `f` as `field(f)` says.
fn wide_arm(count: usize, field: impl Fn(usize) -> &'static str) -> String {
    let fields: Vec<_> = (0..count).map(field).collect();
    format!("V({}) => 1", fields.join(", "))
}

/// The keep-alive override for this test is in `.config/nextest.toml`: without the
/// all-wildcard row cut, and without memoization, the arms meet 2^40 matrices.
#[test]
fn one_named_field_per_arm_checks_in_linear_time() {
    let arms: Vec<_> = (0..40)
        .map(|i| wide_arm(40, |f| if f == i { "X" } else { "_" }))
        .collect();
    clean(&wide(40, &arms));
    // Memoization alone saves the rows above, whose leftovers are alike. Below a first
    // `V(_, ...)`, arms that each need two fields, every first field before every second, meet
    // 2^32 distinct matrices unless the cut stops at that arm inside the `V` bucket.
    let arms: Vec<_> = std::iter::once(wide_arm(64, |_| "_"))
        .chain((0..32).map(|i| wide_arm(64, |f| if f == i || f == 32 + i { "X" } else { "_" })))
        .collect();
    let src = wide(64, &arms);
    // Every arm after the `_`, the last `_ => 0` too, is the `_`'s.
    let expected: Vec<_> = (1..=arms.len())
        .map(|k| (Code::UnreachableArm, (8 + k, 9), vec![(8, 9)]))
        .collect();
    assert_eq!(found(&src), expected);
}

/// The keep-alive override for this test is in `.config/nextest.toml`: without memoization,
/// the `X` and `Y` branches of each field meet the same matrix, 2^39 visits in all.
#[test]
fn repeated_matrices_check_in_linear_time() {
    let arms: Vec<_> = (0..39)
        .flat_map(|i| {
            ["X", "Y"].map(|named| {
                wide_arm(40, move |f| match f {
                    39 => "X",
                    f if f == i => named,
                    _ => "_",
                })
            })
        })
        .collect();
    let src = wide(40, &arms);
    // The arms start on line 8; every arm after the first two is covered by both of them.
    let expected: Vec<_> = (2..arms.len())
        .map(|k| (Code::UnreachableArm, (8 + k, 9), vec![]))
        .collect();
    assert_eq!(found(&src), expected);
}

#[test]
fn let_always_matches() {
    // Each row: the statement on line 14, and where it always matches.
    let rows = [
        ("    if let V(x) = o {}", Some(12)),
        ("    while let V(x) = o {}", Some(15)),
        ("    if let Running(c) = s {}", None),
        ("    if let x = maybe {}", None),
    ];
    for (stmt, col) in rows {
        let expected: Vec<_> = col
            .into_iter()
            .map(|col| (Code::AlwaysMatches, (14, col)))
            .collect();
        codes(&in_fn(stmt), &expected);
    }
}

#[test]
fn broken_pattern_skips_coverage() {
    codes(
        &match_on("e", &["Nmu(n) => n", "Add(_, _) => 1"]),
        &[(Code::UnknownVariant, (13, 9))],
    );
    // A sub-pattern at a field of an unknown type fails too.
    codes(
        &format!(
            "{COVER}\nenum W {{\n    Wrap(Nope)\n    Other\n}}\nfn f(w: W) -> i64 {{\n    match w {{\n        Wrap(_) => 1\n    }}\n}}"
        ),
        &[(Code::UnknownType, (12, 10))],
    );
}

/// `decls` (whole lines), `const LIMIT = 9`, then a function of a `ty` parameter `v` and an `i64`
/// parameter `hi` whose `match` on `v` has each of `arms` on its own line; and the line of the
/// `match` keyword, which is at column 5. The first arm is on the line after it, at column 9.
fn typed(decls: &str, ty: &str, arms: &[&str]) -> (String, usize) {
    let arms = arms
        .iter()
        .map(|arm| format!("        {arm}\n"))
        .collect::<Vec<_>>()
        .concat();
    let src = format!(
        "{decls}const LIMIT = 9\nfn t(v: {ty}, hi: i64) -> i64 {{\n    match v {{\n{arms}    }}\n}}\nfn main() {{}}"
    );
    (src, decls.lines().count() + 3)
}

#[test]
fn literal_patterns() {
    // Each row: the type, the arm before `_ => 0`, and the code it reports with its column.
    let rows = [
        ("u8", "0 => 1", None),
        ("u8", "255 => 1", None),
        ("u8", "256 => 1", Some((Code::IntegerDoesNotFit, 9))),
        ("u8", "-0 => 1", Some((Code::NegatedU8, 9))),
        ("u8", "-1 => 1", Some((Code::NegatedU8, 9))),
        ("i64", "-9223372036854775808 => 1", None),
        ("i64", "9223372036854775807 => 1", None),
        ("i64", "'a' => 1", Some((Code::Mismatch, 9))),
        ("bool", "1 => 1", Some((Code::Mismatch, 9))),
        ("char", "'a' => 1", None),
        ("string", "\"a\" => 1", None),
        ("bool", "true => 1", None),
        ("string", "'a' => 1", Some((Code::Mismatch, 9))),
    ];
    for (ty, arm, code) in rows {
        let (src, line) = typed("", ty, &[arm, "_ => 0"]);
        let expected: Vec<_> = code
            .into_iter()
            .map(|(code, col)| (code, (line + 1, col)))
            .collect();
        codes(&src, &expected);
    }
    // A literal at a position that takes only names is reported as such, whatever its type.
    codes(
        &format!("{DECLS}\nfn f(s: State) {{\n    if let Running(0) = s {{}}\n}}"),
        &[(Code::OnlyNamePosition, (14, 20))],
    );
    // Each literal is the range of one value; a `char` is its code point.
    let (src, _) = typed("", "char", &["'a' => 1", "_ => 0"]);
    contains(&src, &["(arm 'a' (block 1))"]);
    let (src, _) = typed("", "u8", &["7 => 1", "_ => 0"]);
    contains(&src, &["(arm 7 (block 1))"]);
    let (src, _) = typed("", "bool", &["true => 1", "_ => 0"]);
    contains(&src, &["(arm true (block 1))"]);
    let (src, _) = typed("", "string", &["\"a\" => 1", "_ => 0"]);
    contains(&src, &["(arm \"a\" (block 1))"]);
}

/// One below `'\u{E000}'` is a surrogate, which no `char` is.
#[test]
fn exclusive_char_range_should_end_below_the_surrogates_when_it_leaves_out_e000() {
    let (src, _) = typed("", "char", &["'a'..'\\u{E000}' => 1", "_ => 0"]);
    contains(&src, &["(arm 'a'..='\\u{d7ff}' (block 1))"]);
}

#[test]
fn range_patterns() {
    // Each row: the type, the arm before `_ => 0`, and the code it reports with its column.
    let rows = [
        ("u8", "200..=255 => 1", None),
        ("u8", "200..256 => 1", Some((Code::IntegerDoesNotFit, 14))),
        ("string", "\"a\"..\"b\" => 1", Some((Code::BadRange, 9))),
        ("i64", "5..=4 => 1", Some((Code::BadRange, 9))),
        ("i64", "5..5 => 1", Some((Code::BadRange, 9))),
        ("i64", "5..=5 => 1", None),
        ("i64", "0..=\"a\" => 1", Some((Code::BadRange, 9))),
        ("i64", "-9..=-1 => 1", None),
        ("u8", "-1..=3 => 1", Some((Code::NegatedU8, 9))),
        ("char", "'\\u{D7FF}'..='\\u{E000}' => 1", None),
        ("i64", "0..=LIMIT => 1", Some((Code::ConstantInPattern, 13))),
        ("i64", "0..hi => 1", Some((Code::Expected, 12))),
        ("i64", "'a'..='z' => 1", Some((Code::Mismatch, 9))),
        ("bool", "0..=1 => 1", Some((Code::BadRange, 9))),
    ];
    for (ty, arm, code) in rows {
        let (src, line) = typed("", ty, &[arm, "_ => 0"]);
        let expected: Vec<_> = code
            .into_iter()
            .map(|(code, col)| (code, (line + 1, col)))
            .collect();
        codes(&src, &expected);
    }
    let (src, line) = typed("", "i64", &["0..hi => 1", "_ => 0"]);
    diagnostic(&src, Code::Expected, (line + 1, 12, "hi"), &[]);
    // A constant as a bound gets no guard help: `n == LIMIT` would mean another range.
    let (src, _) = typed("", "i64", &["0..=LIMIT => 1", "_ => 0"]);
    assert_eq!(diags(&src)[0].help(), None);
    // `a..b` leaves out `b`.
    let (src, _) = typed("", "i64", &["0..10 => 1", "-5..=-1 => 2", "_ => 0"]);
    contains(&src, &["(arm 0..=9 (block 1)) (arm -5..=-1 (block 2))"]);
}

/// The enums of the alternative tests: lines 1 to 9.
const ALTS: &str = "enum Expr {
    Num(i64)
    Add(Expr, Expr)
}
enum P {
    Pair(i64, string)
    Two(i64, i64)
}
fn main() {}";

#[test]
fn alternatives_bind() {
    let in_t = |arm: &str| {
        format!(
            "{ALTS}\nfn t(e: Expr, p: P) -> i64 {{\n    let _ = match {arm}\n        _ => 0\n    }}\n    0\n}}"
        )
    };
    clean(&in_t("e {\n        Add(Num(0), x) | Add(x, Num(0)) => 0"));
    // `x` is missing from the second alternative, and `y` from the first: one error, at the
    // alternative that lacks a name of the first.
    codes(
        &in_t("e {\n        Add(x, _) | Add(_, y) => 0"),
        &[(Code::AlternativeBindings, (12, 21))],
    );
    let src = in_t("e {\n        Add(x, _) | Add(_, y) => 0");
    diagnostic(&src, Code::AlternativeBindings, (12, 21, "Add(_, y)"), &[]);
    // A name only a later alternative binds is reported at the first, which lacks it.
    diagnostic(
        &in_t("e {\n        Add(_, _) | Add(_, y) => 0"),
        Code::AlternativeBindings,
        (12, 9, "Add(_, _)"),
        &[],
    );
    // Once, though two later alternatives bind it.
    codes(
        &in_t("e {\n        Num(_) | Add(x, _) | Add(_, x) => x"),
        &[(Code::AlternativeBindings, (12, 9))],
    );
    codes(
        &in_t("p {\n        Pair(x, _) | Pair(_, x) => 0"),
        &[(Code::Mismatch, (12, 30))],
    );
    // A name bound twice within one alternative is still reported.
    codes(
        &in_t("p {\n        Two(x, _) | Two(x, x) => 0"),
        &[(Code::BoundTwice, (12, 28))],
    );
    // Each name is one local, which every alternative binds.
    contains(
        &in_t("p {\n        Two(x, 0) | Two(0, x) => x"),
        &["(arm (| (Two x#2 0) (Two 0 x#2)) (block x#2))"],
    );
    // Nested alternatives bind the same names as the arm's.
    clean(&in_t(
        "e {\n        Add(Num(x) | Add(Num(x), _), _) | Add(_, Num(x)) => x",
    ));
    // An alternative at a position that takes only names is reported as such.
    codes(
        &format!("{DECLS}\nfn f(s: State) {{\n    if let Running(a | b) = s {{}}\n}}"),
        &[(Code::OnlyNamePosition, (14, 20))],
    );
}

#[test]
fn guards() {
    contains(
        &match_on("e", &["Num(n) if n > 0 => n", "_ => 0"]),
        &["(arm (Num n#4) (if (> n#4 0)) (block n#4))"],
    );
    codes(
        &match_on("e", &["Num(n) if n => n", "_ => 0"]),
        &[(Code::Mismatch, (13, 19))],
    );
    // A guard sees the arm's bindings, and its own errors fail the `match`.
    codes(
        &match_on("e", &["Num(n) if nope => n", "_ => 0"]),
        &[(Code::BindingNotFound, (13, 19))],
    );
}

/// A `coverage_finite` row: the type, the arms, and the code each reports, by arm.
type CoverageRow<'a> = (&'a str, &'a [&'a str], &'a [(Code, Option<usize>)]);

#[test]
fn coverage_finite() {
    let full_char = "'\\u{0}'..='\\u{D7FF}' | '\\u{E000}'..='\\u{10FFFF}' => 1";
    // Each row: the type, the arms, and the code each reports, at its arm (`Some(k)`, the
    // k-th arm) or at the `match` keyword (`None`).
    let rows: [CoverageRow<'_>; 10] = [
        ("u8", &["0..=127 => 1", "128..=255 => 2"], &[]),
        (
            "u8",
            &["0..=127 => 1", "128..=255 => 2", "_ => 3"],
            &[(Code::UnreachableArm, Some(2))],
        ),
        ("u8", &["0..=127 | 128..=255 => 1"], &[]),
        (
            "u8",
            &["0..=127 | 128..=255 => 1", "_ => 3"],
            &[(Code::UnreachableArm, Some(1))],
        ),
        ("u8", &["0..=9 => 1"], &[(Code::NonExhaustive, None)]),
        (
            "i64",
            &[
                "-9223372036854775808..=-1 => 1",
                "0 => 2",
                "1..=9223372036854775807 => 3",
            ],
            &[],
        ),
        ("char", &[full_char], &[]),
        (
            "char",
            &["'\\u{0}'..='\\u{D7FF}' => 1"],
            &[(Code::NonExhaustive, None)],
        ),
        ("bool", &["true => 1"], &[(Code::NonExhaustive, None)]),
        ("string", &["\"a\" => 1"], &[(Code::NonExhaustive, None)]),
    ];
    for (ty, arms, expected) in rows {
        let (src, line) = typed("", ty, arms);
        let expected: Vec<_> = expected
            .iter()
            .map(|&(code, arm)| match arm {
                Some(k) => (code, (line + 1 + k, 9)),
                None => (code, (line, 5)),
            })
            .collect();
        codes(&src, &expected);
    }
}

#[test]
fn reachability() {
    // After `1..=3` and `4..=6`: `5` is the second arm's, `2..=5` is no one arm's, and `0..=5`
    // reaches `0`.
    let rows = [
        ("5 => 3", vec![(Code::UnreachableArm, (6, 9), vec![(5, 9)])]),
        ("2..=5 => 3", vec![(Code::UnreachableArm, (6, 9), vec![])]),
        ("0..=5 => 3", vec![]),
    ];
    for (arm, expected) in rows {
        let (src, _) = typed("", "i64", &["1..=3 => 1", "4..=6 => 2", arm, "_ => 0"]);
        assert_eq!(found(&src), expected, "{src}");
    }
    let pair = "enum P { Pair(i64, i64) }\n";
    let (src, _) = typed(
        pair,
        "P",
        &["Pair(a, _) | Pair(_, a) if a > 0 => a", "_ => 0"],
    );
    clean(&src);
    let expr = "enum Expr {\n    Num(i64)\n    Add(Expr, Expr)\n}\n";
    // An alternative covered by an earlier one of its own arm names no covering arm.
    let (src, line) = typed(expr, "Expr", &["Num(1) | Num(1) => 0", "_ => 1"]);
    assert_eq!(
        found(&src),
        [(Code::UnreachableArm, (line + 1, 18), vec![])]
    );
    let (src, line) = typed(expr, "Expr", &["Add(Num(0) | Num(0), x) => 0", "_ => 1"]);
    assert_eq!(
        found(&src),
        [(Code::UnreachableArm, (line + 1, 22), vec![])]
    );
    // The nested `true` is the first arm's; `false` and `W` are reached.
    let vw = "enum E {\n    V(bool)\n    W\n}\n";
    let (src, line) = typed(vw, "E", &["V(true) => 0", "V(true | false) | W => 1"]);
    assert_eq!(
        found(&src),
        [(Code::UnreachableArm, (line + 2, 11), vec![(line + 1, 9)])]
    );
    // Two alternatives of one arm take a later row's values in different leaves, a level below
    // the `|`: that arm covers it, as a whole arm or as an alternative.
    let (src, line) = typed(vw, "E", &["V(true) | V(false) => 0", "W => 1", "V(_) => 2"]);
    assert_eq!(
        found(&src),
        [(Code::UnreachableArm, (line + 3, 9), vec![(line + 1, 9)])]
    );
    let (src, line) = typed(vw, "E", &["V(true) | _ => 0", "V(_) => 1"]);
    assert_eq!(
        found(&src),
        [(Code::UnreachableArm, (line + 2, 9), vec![(line + 1, 9)])]
    );
    let (src, line) = typed(vw, "E", &["V(true) | V(false) => 0", "W | V(_) => 1"]);
    assert_eq!(
        found(&src),
        [(Code::UnreachableArm, (line + 2, 13), vec![(line + 1, 9)])]
    );
    // Two rows of one arm take the `_`'s values: one arm covers it.
    let ab = "enum AB {\n    A\n    B\n}\n";
    let (src, line) = typed(ab, "AB", &["A | B => 0", "_ => 1"]);
    assert_eq!(
        found(&src),
        [(Code::UnreachableArm, (line + 2, 9), vec![(line + 1, 9)])]
    );
    let vbb = "enum P { V(bool, bool) }\n";
    let (src, line) = typed(vbb, "P", &["V(true | false, true | false) => 0", "_ => 1"]);
    assert_eq!(
        found(&src),
        [(Code::UnreachableArm, (line + 2, 9), vec![(line + 1, 9)])]
    );
    // Each arm's own alternatives are marked, though the second arm's bucket repeats the first's.
    let q = "enum XY {\n    X\n    Y\n}\nenum Q { Q(XY, bool) }\n";
    let (src, _) = typed(
        q,
        "Q",
        &["Q(X, true | false) => 0", "Q(Y, true | false) => 1"],
    );
    clean(&src);
    // An alternative covered by an earlier arm names it; the fix removes it with its `|`.
    let (src, line) = typed(ab, "AB", &["A => 0", "A | B => 1"]);
    let d = diagnostic(
        &src,
        Code::UnreachableArm,
        (line + 2, 9, "A"),
        &[(line + 1, 9)],
    );
    assert_eq!(
        fixes(&src, &d),
        [(false, vec![(line + 2, 9, "A | ", String::new())])]
    );
    clean(&applied(&src, &d));
    let (src, line) = typed(ab, "AB", &["B => 0", "A | B => 1"]);
    let d = diagnostic(
        &src,
        Code::UnreachableArm,
        (line + 2, 13, "B"),
        &[(line + 1, 9)],
    );
    assert_eq!(
        fixes(&src, &d),
        [(false, vec![(line + 2, 10, " | B", String::new())])]
    );
    clean(&applied(&src, &d));
}

#[test]
fn cut_should_take_unexpanded_alternatives() {
    let vbb = "enum P { V(bool, bool) }\n";
    // Where the first arm has only wildcards left, it takes the `|` below it unexpanded: `true` is
    // the first arm's there and the second's where the `|` is expanded, so no one arm covers it.
    let (src, line) = typed(
        vbb,
        "P",
        &[
            "V(true, _) => 0",
            "V(false, true) => 1",
            "V(_, true | false) => 2",
        ],
    );
    assert_eq!(
        found(&src),
        [(Code::UnreachableArm, (line + 3, 14), vec![])]
    );
    // An arm takes `false` both unexpanded and expanded, as two rows of its own `|`.
    let (src, line) = typed(
        vbb,
        "P",
        &[
            "V(true, _) | V(false, false) => 0",
            "V(_, false | true) => 1",
        ],
    );
    assert_eq!(
        found(&src),
        [(Code::UnreachableArm, (line + 2, 14), vec![(line + 1, 9)])]
    );
    // The cut takes the `|` inside a `Some` too: `1` is the first arm's there and the second's
    // where the `|` is expanded.
    let (src, line) = typed(
        "enum P { V(bool, i64?) }\n",
        "P",
        &[
            "V(true, _) => 0",
            "V(false, Some(1)) => 1",
            "V(_, Some(1 | 2)) => 2",
            "_ => 3",
        ],
    );
    assert_eq!(
        found(&src),
        [(Code::UnreachableArm, (line + 3, 19), vec![])]
    );
    // The top matrix is a cut: the whole arm below the wildcard arm is its first row's.
    let (src, line) = typed("", "bool", &["_ => 0", "true | false => 1"]);
    assert_eq!(
        found(&src),
        [(Code::UnreachableArm, (line + 2, 9), vec![(line + 1, 9)])]
    );
    // The `|` the cut takes sits in a field, and in an alternative: `R` is the first arm's where
    // the cut is and the second's elsewhere.
    let k =
        "enum C {\n    R\n    S\n    T\n}\nenum P {\n    W(C)\n    Z\n}\nenum K { K(bool, P) }\n";
    for third in ["K(_, W(R | S)) => 2", "K(_, W(R | S) | Z) => 2"] {
        let (src, line) = typed(
            k,
            "K",
            &["K(true, _) => 0", "K(false, W(R)) => 1", third, "_ => 3"],
        );
        assert_eq!(
            found(&src),
            [(Code::UnreachableArm, (line + 3, 16), vec![])],
            "{third}"
        );
    }
}

#[test]
fn guards_cover_nothing() {
    let rows: [&[&str]; 3] = [
        &["Num(n) if n > 0 => n", "Add(_, _) => 1", "Mul(_, _) => 2"],
        &["Num(0) => 0", "Add(_, _) => 1", "Mul(_, _) => 2"],
        &[
            "Num(n) if n > 0 => n",
            "Num(0) => 0",
            "Add(_, _) => 1",
            "Mul(_, _) => 2",
        ],
    ];
    for arms in rows {
        codes(&match_on("e", arms), &[(Code::NonExhaustive, (12, 5))]);
    }
    let ab = "enum AB {\n    A\n    B\n}\nenum P { P(AB, i64) }\n";
    let (src, line) = typed(ab, "P", &["P(B, _) => 2", "P(A, x) if x > 0 => 1"]);
    // The first arm with `P` at the top covers only part of it; `P(A, _)` is missing, and a
    // guarded arm may not match it (`check::exhaust::tests`, the `witnesses_should_name...` row).
    diagnostic(
        &src,
        Code::NonExhaustive,
        (line, 5, "match"),
        &[(line + 1, 9)],
    );
}

/// The help that names `_ =>` comes with a gap in an integer, `char` or `string` type, not with
/// a missing variant or `bool`.
#[test]
fn gap_help() {
    let rows: [(&str, &[&str], bool); 4] = [
        ("u8", &["0..=9 => 1"], true),
        ("string", &["\"a\" => 1"], true),
        ("bool", &["true => 1"], false),
        ("i64", &["n if n > 0 => 1"], false),
    ];
    for (ty, arms, help) in rows {
        let (src, line) = typed("", ty, arms);
        let d = diagnostic(&src, Code::NonExhaustive, (line, 5, "match"), &[]);
        assert_eq!(d.help().is_some(), help, "{src}");
    }
}

/// The guard helps of a constant in a pattern and of a name bound twice.
#[test]
fn guard_helps() {
    let src = arm_on_15("n", "LIMIT => 1").replace("enum Expr", "const LIMIT = 9\nenum Expr");
    let d = &diags(&src)[0];
    assert_eq!(d.code(), Code::ConstantInPattern);
    assert!(d.help().is_some());
    let d = &diags(&arm_on_15("e", "Add(x, x) => 1"))[0];
    assert!(d.help().is_some());
    // Outside a `match` there is no arm to take a guard.
    let d = &diags(&in_fn("    if let Add(x, x) = e {}"))[0];
    assert_eq!(d.code(), Code::BoundTwice);
    assert_eq!(d.help(), None);
}

/// The keep-alive override for this test is in `.config/nextest.toml`: expanding alternatives
/// eagerly, or without memoization, meets 2^40 rows.
#[test]
fn forty_alternatives() {
    let fields = vec!["bool"; 40].join(", ");
    let pattern = vec!["true | false"; 40].join(", ");
    clean(&format!(
        "enum F {{ V({fields}) }}\nfn f(x: F) -> i64 {{\n    match x {{\n        V({pattern}) => 1\n    }}\n}}\nfn main() {{}}"
    ));
}
