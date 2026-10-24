//! Enum declarations: variants and payloads, the reserved names, which enums can be built, the
//! boxes that break a cycle, and which enums are counted.

use std::fmt::Write as _;

use super::*;

/// `decls` with an empty `main`.
fn with_main(decls: &str) -> String {
    format!("{decls}\nfn main() {{}}")
}

/// The declarations, and the codes and positions they report.
type Row<'a> = (&'a str, &'a [(Code, (usize, usize))]);

#[test]
fn enum_declarations_lower() {
    let rows = [
        (
            "enum Shape {\n    Circle(radius: i64)\n    Rect(w: i64, h: i64)\n}",
            "(enum Shape (Circle i64) (Rect i64 i64))",
        ),
        (
            "class C { mut n: i64 }\nenum State {\n    Idle\n    Running(C)\n}",
            "(enum State Idle (Running C))",
        ),
        (
            "enum Expr {\n    Num(i64)\n    Add(Expr, Expr)\n}",
            "(enum Expr (Num i64) (Add (box Expr) (box Expr)))",
        ),
        // Each payload owns its labels.
        (
            "enum Shape {\n    Rect(w: i64, h: i64)\n    Square(w: i64)\n}",
            "(enum Shape (Rect i64 i64) (Square i64))",
        ),
    ];
    for (decls, part) in rows {
        contains(&with_main(decls), &[part]);
    }
}

#[test]
fn boxes_follow_components() {
    // Each row: the declarations, the printed enums, and each enum's cycle, by enum.
    let rows: [(&str, &[&str], &[bool]); 4] = [
        (
            "enum A { X(B) }\nenum B {\n    Y(A)\n    Z\n}",
            &["(enum A (X (box B)))", "(enum B (Y (box A)) Z)"],
            &[true, true],
        ),
        (
            "enum L {\n    Nil\n    Cons(i64, L?)\n}",
            &["(enum L Nil (Cons i64 (box L?)))"],
            &[true],
        ),
        (
            "enum T {\n    Leaf\n    Node([T])\n}",
            &["(enum T Leaf (Node [T]))"],
            &[false],
        ),
        (
            "enum P { Q(Inner) }\nenum Inner { R(i64) }",
            &["(enum P (Q Inner))", "(enum Inner (R i64))"],
            &[false, false],
        ),
    ];
    for (decls, parts, cycles) in rows {
        let src = with_main(decls);
        contains(&src, parts);
        let program = checked(&src).0.expect("the row should check clean");
        let found: Vec<bool> = program.enums.iter().map(|e| e.cycle.is_some()).collect();
        assert_eq!(found, cycles, "{decls}");
    }
    // `A` and `B` share one component, so one cycle.
    let program = checked(&with_main(rows[0].0))
        .0
        .expect("A and B should check clean");
    assert_eq!(program.enums[0].cycle, program.enums[1].cycle);
}

#[test]
fn enum_declaration_errors() {
    let rows: [(&str, _); 9] = [
        ("enum D {\n    north\n}", (Code::VariantCase, (2, 5))),
        (
            "enum M {\n    Move(i64, to: i64)\n}",
            (Code::MixedLabels, (2, 15)),
        ),
        ("enum Empty {}", (Code::EmptyEnum, (1, 6))),
        ("enum E {\n    A\n    A\n}", (Code::DefinedTwice, (3, 5))),
        (
            "enum E {\n    V(x: i64, x: i64)\n}",
            (Code::DefinedTwice, (2, 15)),
        ),
        ("enum E {\n    V(Nope)\n}", (Code::UnknownType, (2, 7))),
        ("enum E { A }\nclass E {}", (Code::DefinedTwice, (2, 7))),
        ("class E {}\nenum E { A }", (Code::DefinedTwice, (2, 6))),
        (
            "enum E {\n    A\n    fn A(self) {}\n}",
            (Code::DefinedTwice, (3, 8)),
        ),
    ];
    for (decls, expected) in rows {
        codes(&with_main(decls), &[expected]);
    }
}

#[test]
fn reserved_names() {
    let rows: [Row<'_>; 8] = [
        ("class Result {}", &[(Code::BuiltInName, (1, 7))]),
        ("fn Option() {}", &[(Code::BuiltInName, (1, 4))]),
        ("enum E {\n    Ok(i64)\n}", &[(Code::BuiltInName, (2, 5))]),
        (
            "fn g() {\n    let Some = 1\n}",
            &[(Code::BuiltInName, (2, 9))],
        ),
        ("fn f(Err: i64) {}", &[(Code::BuiltInName, (1, 6))]),
        ("class C {\n    Ok: i64\n    fn Some(self) {}\n}", &[]),
        // The item and the binding are still declared, so their uses do not cascade.
        (
            "class Result {}\nfn f(r: Result) {}",
            &[(Code::BuiltInName, (1, 7))],
        ),
        (
            "fn g() {\n    let Err = 1\n    print(Err)\n}",
            &[(Code::BuiltInName, (2, 9))],
        ),
    ];
    for (decls, expected) in rows {
        codes(&with_main(decls), expected);
    }
}

#[test]
fn enum_named_local() {
    let rows = [
        (
            "enum Shape { Dot }\nfn g() {\n    let Shape = 1\n    print(Shape)\n}",
            (3, 9),
        ),
        ("enum Shape { Dot }\nfn f(Shape: i64) {}", (2, 6)),
        (
            "enum Shape { Dot }\nfn g() {\n    for Shape in [1] {}\n}",
            (3, 9),
        ),
    ];
    for (decls, at) in rows {
        codes(&with_main(decls), &[(Code::EnumNamedLocal, at)]);
    }
}

#[test]
fn never_built() {
    let rows: [Row<'_>; 11] = [
        (
            "enum Bad { Wrap(Bad) }",
            &[(Code::NeverConstructed, (1, 6))],
        ),
        // A field of an unknown type has its own error, and the chain walks past it.
        (
            "enum Bad { Wrap(Nope, Bad) }",
            &[
                (Code::NeverConstructed, (1, 6)),
                (Code::UnknownType, (1, 17)),
            ],
        ),
        // The chain walks past fields of other types, and past a type it reached already.
        (
            "enum Bad { Wrap(i64, Fine, Bad) }\nenum Fine { A }",
            &[(Code::NeverConstructed, (1, 6))],
        ),
        (
            "enum D { Q(K, K) }\nclass K {\n    d: D\n    x: Nope\n}",
            &[
                (Code::NeverConstructed, (1, 6)),
                (Code::UnknownType, (4, 8)),
            ],
        ),
        (
            "enum D { Q(K) }\nclass K { d: D }",
            &[(Code::NeverConstructed, (1, 6))],
        ),
        (
            "enum C { Z(Bad) }\nenum Bad { Wrap(Bad) }",
            &[(Code::NeverConstructed, (2, 6))],
        ),
        // An enum whose error is reported already is not reported again.
        ("enum E {}\nenum F { G(E) }", &[(Code::EmptyEnum, (1, 6))]),
        // The empty enum's node lies past the classes and the enums before it.
        (
            "class C { n: i64 }\nenum A { X }\nenum E {}\nenum F { G(E) }",
            &[(Code::EmptyEnum, (3, 6))],
        ),
        ("enum A { X(B) }\nenum B {\n    Y(A)\n    Z\n}", &[]),
        ("enum L { Cons([L]) }", &[]),
        ("enum L { Cons(L?) }", &[]),
    ];
    for (decls, expected) in rows {
        codes(&with_main(decls), expected);
    }
}

#[test]
fn variant_case_fix() {
    // Each row: the variant, and the name its fix gives, if any.
    let rows = [
        ("north", Some("North")),
        ("_foo", None),
        ("_1x", None),
        ("_1", None),
    ];
    for (variant, fixed) in rows {
        let src = with_main(&format!("enum D {{\n    {variant}\n}}"));
        let d = diagnostic(&src, Code::VariantCase, (2, 5, variant), &[]);
        let expected: Vec<FixAt<'_>> = fixed
            .map(|f| (false, vec![(2, 5, variant, f.to_owned())]))
            .into_iter()
            .collect();
        assert_eq!(fixes(&src, &d), expected, "{variant}");
    }
}

#[test]
fn long_declaration_chain_checks() {
    // `components` keeps its own stack: a walk that recursed once per enum would need far more
    // than the test thread's stack for this chain.
    const N: usize = 50_000;
    let mut decls = String::new();
    for i in 0..N - 1 {
        writeln!(decls, "enum E{i} {{ A(E{}) }}", i + 1)
            .expect("writing to a string should not fail");
    }
    write!(decls, "enum E{} {{ A }}", N - 1).expect("writing to a string should not fail");
    codes(&with_main(&decls), &[]);
}

#[test]
fn class_cycle_not_repeated() {
    codes(
        &with_main("class N { next: N }\nenum E { V(N) }"),
        &[(Code::NeverConstructed, (1, 11))],
    );
    // `N` and `E` need each other, but only through `N`'s own cycle.
    codes(
        &with_main("class N {\n    next: N\n    e: E\n}\nenum E { V(N) }"),
        &[(Code::NeverConstructed, (2, 5))],
    );
}

/// The enums and the class the construction tests build and name.
const BUILT: &str = "enum Expr {
    Num(i64)
    Add(Expr, Expr)
    fn eval(self) -> i64 {
        0
    }
}
enum State {
    Idle
    Running(i64)
}
enum Light {
    Idle
    On
}
enum Shape {
    Rect(w: i64, h: i64)
    Boxed(Size)
    Pair([i64], i64)
}
class Size {
    width: i64
    height: i64
}";

/// `stmt` as the one statement of `g`, which takes an `Expr` `e` and a `Shape` `s`, beside the
/// declarations of `BUILT`: `stmt` starts at line 2, column 5.
fn in_g(stmt: &str) -> String {
    format!("fn g(e: Expr, s: Shape) {{\n    {stmt}\n}}\n{BUILT}\nfn main() {{}}")
}

#[test]
fn construction_lowers() {
    let rows = [
        ("Expr.Num(2)", "(variant Expr.Num 2)"),
        (
            "Expr.Add(Expr.Num(2), Expr.Num(3))",
            "(variant Expr.Add (variant Expr.Num 2) (variant Expr.Num 3))",
        ),
        ("State.Idle", "(variant State.Idle)"),
        ("Shape.Rect(w: 2, h: 3)", "(variant Shape.Rect 2 3)"),
        (
            "Shape.Boxed(Size(width: 2, height: 3))",
            "(variant Shape.Boxed (new Size 2 3))",
        ),
    ];
    for (value, printed) in rows {
        contains(&in_g(&format!("let _ = {value}")), &[printed]);
    }
    // A variant lives inside its enum, so a class of its name is another thing.
    let src = format!(
        "class Num {{}}\nfn main() {{\n    let _ = Num()\n    let _ = Expr.Num(1)\n}}\n{BUILT}"
    );
    contains(&src, &["(new Num) (variant Expr.Num 1)"]);
}

#[test]
fn construction_spills() {
    // A counted argument before one that exits is spilled, as a constructor's is.
    let src = format!(
        "fn f(xs: [i64], c: bool) -> i64 {{\n    let _ = Shape.Pair(xs, if c {{ return 1 }} else {{ 2 }})\n    0\n}}\n{BUILT}\nfn main() {{}}"
    );
    contains(
        &src,
        &[
            "(block (let #2 xs#0) (let #3 (if c#1 (block (return 1)) (block 2))) (variant Shape.Pair #2 #3))",
        ],
    );
}

#[test]
fn construction_errors() {
    // Each row: the statement, and the code it reports at the column of line 2.
    let rows = [
        ("let _ = Num(2)", Code::BareVariant, 13),
        ("let _ = Idle", Code::BareVariant, 13),
        ("let _ = Expr.Nmu(2)", Code::UnknownVariant, 18),
        ("let _ = State.Idle()", Code::VariantForm, 23),
        ("let _ = Expr.Num", Code::VariantForm, 18),
        ("let _ = Expr.Num(n: 2)", Code::LabeledArgument, 22),
        ("let _ = Expr.Num(1, 2)", Code::ArgumentCount, 13),
        ("let _ = Shape.Rect(h: 3, w: 2)", Code::FieldOutOfOrder, 24),
        ("let _ = Shape.Rect(w: 2)", Code::MissingField, 13),
        ("let _ = Shape.Rect(2, 3)", Code::ArgumentNeedsLabel, 24),
        (
            "let _ = Shape.Rect(w: 2, h: 3, d: 4)",
            Code::TooManyArguments,
            36,
        ),
        ("let _ = Shape.Rect(w: 2, x: 3)", Code::NoMember, 30),
        ("let _ = e.Num(1)", Code::MemberKind, 15),
        ("let _ = Expr.eval(e)", Code::MemberKind, 18),
        ("let _ = s.w", Code::EnumField, 15),
        ("s.w = 5", Code::EnumField, 7),
        // A method read as a field, and a method no enum declares, as on a class.
        ("let _ = e.eval", Code::MethodNotField, 15),
        ("let _ = e.nope()", Code::NoMember, 15),
        // A name no enum has as a variant is no bare variant.
        ("let _ = nope", Code::BindingNotFound, 13),
        // An enum's name, like a class's, is a type, not a value.
        ("let _ = Shape", Code::TypeAsValue, 13),
    ];
    for (stmt, code, col) in rows {
        codes(&in_g(stmt), &[(code, (2, col))]);
    }
}

/// A construction that fails in itself is `Error`; one whose argument fails keeps the enum's
/// type, as a class's construction does. Each row binds it to a `bool`.
#[test]
fn failed_construction_poisons() {
    let rows = [
        ("Shape.Rect(w: 2)", &[(Code::MissingField, (2, 19))][..]),
        ("Expr.Num(n: 2)", &[(Code::LabeledArgument, (2, 28))]),
        (
            "Expr.Num(nope)",
            &[(Code::Mismatch, (2, 19)), (Code::BindingNotFound, (2, 28))],
        ),
        (
            "Size(width: nope, height: 1)",
            &[(Code::Mismatch, (2, 19)), (Code::BindingNotFound, (2, 31))],
        ),
    ];
    for (value, expected) in rows {
        codes(&in_g(&format!("let _: bool = {value}")), expected);
    }
}

/// An item that takes a variant's name is what the name means.
#[test]
fn bare_variant_should_yield_to_items() {
    // A class's name, and an enum's.
    for name in ["Size", "Shape"] {
        let src = format!(
            "fn main() {{\n    let _ = {name}\n}}\nenum V {{\n    Size\n    Shape\n}}\n{BUILT}"
        );
        codes(&src, &[(Code::TypeAsValue, (2, 13))]);
    }
    // A binding, a function and a constant each take the name of `State.Idle` as they would
    // without the enum: each row is the statement, and the codes it reports from line 2.
    let rows: [Row; 4] = [
        (
            "let Idle = 1\n    let _ = Idle(2)",
            &[(Code::FunctionNotFound, (3, 13))],
        ),
        ("let Idle = 1\n    let _ = Idle", &[]),
        (
            "let _ = Idle\n}\nfn Idle() {",
            &[(Code::TypeAsValue, (2, 13))],
        ),
        (
            "let _ = Idle()\n}\nconst Idle = 1\nfn h() {",
            &[(Code::FunctionNotFound, (2, 13))],
        ),
    ];
    for (stmt, expected) in rows {
        codes(&in_g(stmt), expected);
    }
}

/// `State.Idle()` holds nothing: the fix deletes the call's own parentheses, safely only when
/// they hold nothing: arguments' own diagnostics and comments would go with them.
#[test]
fn variant_form_fix() {
    // Each row: the statement, the column of line 2 and text of the label, and the fix's safety.
    let rows = [
        ("let _ = State.Idle()", 23, "()", true),
        ("let _ = (State.Idle())", 24, "()", true),
        ("let _ = State.Idle(1)", 23, "(1)", false),
        (
            "let _ = State.Idle(// note\n    )",
            23,
            "(// note\n    )",
            false,
        ),
    ];
    for (stmt, col, text, safe) in rows {
        let src = in_g(stmt);
        let d = diagnostic(&src, Code::VariantForm, (2, col, text), &[]);
        assert_eq!(
            fixes(&src, &d),
            [(safe, vec![(2, col, text, String::new())])],
            "{stmt}"
        );
        if safe {
            fix_removes_its_code(&src, &d);
        }
    }
}

/// An unlabeled payload given labels: one diagnostic, whose unsafe fix deletes every label.
#[test]
fn variant_labels_fix() {
    let src = in_g("let _ = Expr.Add(a: e, b: e)");
    let d = diagnostic(&src, Code::LabeledArgument, (2, 22, "a"), &[]);
    assert_eq!(
        fixes(&src, &d),
        [(
            false,
            vec![(2, 22, "a: ", String::new()), (2, 28, "b: ", String::new())]
        )]
    );
}

/// `Math` names the enum, so `Math.sqrt` is one of its variants, not the math idiom.
#[test]
fn enum_named_math() {
    codes(
        "enum Math {\n    Pi\n}\nfn main() {\n    let _ = Math.sqrt(2)\n}",
        &[(Code::UnknownVariant, (5, 18))],
    );
}

/// A variant whose payload mixes labels is reported at its declaration; a construction of it
/// checks only its arguments.
#[test]
fn mixed_labels_poison_construction() {
    codes(
        "enum E {\n    V(a: i64, bool)\n}\nfn main() {\n    let _: bool = E.V(1, nope)\n}",
        &[
            (Code::MixedLabels, (2, 15)),
            (Code::BindingNotFound, (5, 26)),
        ],
    );
}

#[test]
fn enum_methods() {
    contains(
        "enum E {\n    A\n    B(i64)\n    fn same(self, o: E) -> bool { self == o }\n}\nfn main() {\n    let _ = E.A.same(E.B(1))\n}",
        &["(call E.same (variant E.A) (variant E.B 1))"],
    );
    clean("enum E {\n    A\n    fn f(self) -> E { self }\n}\nfn main() {}");
}

#[test]
fn enum_name_shadowed() {
    // The local takes no enum's name, so `Shape.Dot` is still the variant.
    codes(
        "enum Shape {\n    Dot\n    Circle(i64)\n}\nfn main() {\n    let Shape = 1\n    let _ = Shape.Dot\n    let _ = Shape.Circle(2)\n}",
        &[(Code::EnumNamedLocal, (6, 9))],
    );
}

#[test]
fn enum_value_rules() {
    // Each row: the statement, and the code it reports at the column of line 2. An enum gets the
    // code a class gets for each rule.
    let rows = [
        ("let _ = e is e", Code::IsNeedsObjects, 13),
        ("let _ = o is o", Code::IsNeedsObjects, 13),
        ("let _ = e < e", Code::Mismatch, 13),
        ("let _ = p < p", Code::Mismatch, 13),
        ("print(e)", Code::PrintArgument, 11),
        ("print(p)", Code::PrintArgument, 11),
        ("let _ = \"{e}\"", Code::CannotInterpolate, 15),
        ("let _ = \"{p}\"", Code::CannotInterpolate, 15),
        ("let _ = e == f", Code::CannotCompare, 13),
    ];
    let decls = "enum E {\n    A(string)\n    B\n}\nenum F { C }\nclass P { x: i64 }";
    for (stmt, code, col) in rows {
        let src =
            format!("fn g(e: E, o: E?, f: F, p: P) {{\n    {stmt}\n}}\n{decls}\nfn main() {{}}");
        codes(&src, &[(code, (2, col))]);
    }
    for value in ["State.Idle", "P(x: 1)"] {
        let src = format!(
            "const S = {value}\nenum State {{ Idle }}\nclass P {{ x: i64 }}\nfn main() {{}}"
        );
        codes(&src, &[(Code::ConstantNotKnown, (1, 11))]);
    }
}

/// A bare variant in parentheses: the fix replaces the whole name, parentheses included, and
/// the fixed source parses no worse than the source.
#[test]
fn parenthesized_bare_variant_fix_should_replace_the_parentheses() {
    // Each row: the value, the declarations and the enums that have the variant.
    let rows = [
        ("(Red)", "enum E {\n    Red\n}", &["E"][..]),
        ("((Red))", "enum E {\n    Red\n}", &["E"]),
        (
            "(Red)",
            "enum E {\n    Red\n}\nenum F {\n    Red\n}",
            &["E", "F"],
        ),
    ];
    for (value, decls, enums) in rows {
        let src = format!("{decls}\nfn main() {{\n    let _ = {value}\n}}");
        let line = decls.lines().count() + 2;
        let d = diagnostic(&src, Code::BareVariant, (line, 13, value), &[]);
        let safe = enums.len() == 1;
        let expected: Vec<_> = enums
            .iter()
            .map(|e| (safe, vec![(line, 13, value, format!("{e}.Red"))]))
            .collect();
        assert_eq!(fixes(&src, &d), expected, "{value}");
        for fix in d.fixes() {
            let fixed = fix.apply(&src);
            assert!(
                crate::front_end::syntax_errors(&fixed) <= crate::front_end::syntax_errors(&src),
                "{fixed}"
            );
        }
        if safe {
            fix_removes_its_code(&src, &d);
        }
    }
}
