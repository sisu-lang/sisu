//! Optionals of optionals: each operation takes one layer at a time.

use super::generics::{infers, with_decls, with_generics};
use super::*;
use crate::front_end::{PRELUDE_SOURCE, prelude_program};

/// `body` on lines 2 on of `fn g` over `x: i64??`, `w: i64?`, `c: bool`, `a: C?`, `aa: C??`
/// and `p, q: Node??`, then the classes `C`, whose field `b` is an `i64??`, and `Node`, and an
/// empty `main`.
fn with_body(body: &str) -> String {
    format!(
        "fn g(x: i64??, w: i64?, c: bool, a: C?, aa: C??, p: Node??, q: Node??) {{\n    {body}\n}}\nclass C {{\n    b: i64??\n}}\nclass Node {{\n    n: i64\n}}\nfn main() {{}}"
    )
}

/// The type of the first expression of `src`'s `tir` that is written as `text`, as a program
/// writes it. `src` checks clean.
fn type_at(src: &str, text: &str) -> String {
    let (program, ds) = checked(src);
    assert!(ds.is_empty(), "{ds:?}");
    let program = program.expect("a program without diagnostics should have a tir");
    let mut exprs = Vec::new();
    for function in &program.functions {
        function.body.exprs(&mut exprs);
    }
    let found = exprs
        .into_iter()
        .find(|e| &src[e.span.range()] == text)
        .unwrap_or_else(|| panic!("`{text}` should be an expression of {src}"));
    program.type_name(found.ty)
}

/// Conversion wraps one layer, `??`, `if let` and `?.` take one off, and a join or `==` takes
/// the side with more layers.
#[test]
fn one_layer_should_be_taken_at_a_time() {
    for (body, text, ty) in [
        ("let r: i64?? = None", "None", "i64??"),
        ("let y: i64? = 5\n    let r: i64?? = y", "y", "i64??"),
        ("let r = x ?? 5", "x ?? 5", "i64?"),
        ("let r = x ?? None", "x ?? None", "i64?"),
        ("let r = (x ?? None) ?? 0", "(x ?? None) ?? 0", "i64"),
        ("if let v = x {\n        let r = v\n    }", "v", "i64?"),
        (
            "while let v = x {\n        let r = v\n        break\n    }",
            "v",
            "i64?",
        ),
        ("let r = a?.b", "a?.b", "i64??"),
        ("let r = x == None", "None", "i64??"),
        (
            "let r = if c { w } else { None }",
            "if c { w } else { None }",
            "i64?",
        ),
        ("let r = [w, None]", "[w, None]", "[i64?]"),
        // Each spelling of `None` takes its type from the other side, or brings its own.
        (
            "let r = [Some(1), Option.None]",
            "[Some(1), Option.None]",
            "[i64?]",
        ),
        ("let r = [Option.None, w]", "[Option.None, w]", "[i64?]"),
        (
            "let r = [w, Option<i64>.None]",
            "[w, Option<i64>.None]",
            "[i64?]",
        ),
        ("let r = x == Option.None", "Option.None", "i64??"),
        ("let r = Option.None == x", "Option.None", "i64??"),
        (
            "let r = Option<i64?>.None == x",
            "Option<i64?>.None",
            "i64??",
        ),
        ("let r = w == 5", "5", "i64?"),
        ("let r = x == w", "w", "i64??"),
        ("let r = w == x", "w", "i64??"),
        (
            "let r = if c { w } else { x }",
            "if c { w } else { x }",
            "i64??",
        ),
        (
            "let r = if c { x } else { w }",
            "if c { x } else { w }",
            "i64??",
        ),
    ] {
        assert_eq!(type_at(&with_body(body), text), ty, "{body}");
    }
}

/// A value converts through one `?` only, and the side of `??` or `==` with fewer layers is held
/// to the other. A help names the rule where the types differ only in layers.
/// `tests/diagnostics/E0032-nested-optional` pins the wording.
#[test]
fn a_value_should_convert_through_one_layer_only() {
    for (body, at, layers) in [
        ("let r: i64?? = 5", (2, 20, "5"), true),
        ("let r: i64??? = 5", (2, 21, "5"), true),
        ("let r = x ?? \"a\"", (2, 18, "\"a\""), false),
        ("let r = x == 5", (2, 18, "5"), true),
        // A join of three depths: the `i64` is two layers below the join, in either order.
        ("let n = 1\n    let r = [n, w, x]", (3, 14, "n"), true),
        ("let n = 1\n    let r = [x, w, n]", (3, 20, "n"), true),
        (
            "let n = 1\n    let r = match n {\n        0 => n\n        1 => w\n        _ => x\n    }",
            (4, 14, "n"),
            true,
        ),
        (
            "let n = 1\n    let r = match n {\n        0 => x\n        1 => w\n        _ => n\n    }",
            (6, 14, "n"),
            true,
        ),
    ] {
        let d = diagnostic(&with_body(body), Code::Mismatch, at, &[]);
        assert_eq!(d.help().is_some(), layers, "{body}");
    }
}

/// `?.push` takes one layer off, as any `?.` does: on an optional of an optional, whether of an
/// object or of an array, it asks for a layer to be peeled, and pushes nothing.
#[test]
fn push_through_an_optional_of_an_optional_should_be_peeled_first() {
    let object =
        "class C {\n    mut items: [i64]\n}\nfn g(aa: C??) { aa?.items?.push(1) }\nfn main() {}";
    diagnostic(object, Code::MayBeNone, (4, 17, "aa"), &[]);
    let array = "fn g(xs: [i64]??) { xs?.push(1) }\nfn main() {}";
    diagnostic(array, Code::MayBeNone, (1, 21, "xs"), &[]);
}

/// `?.` and `is` on an optional of an optional ask for a layer to be peeled, with no fix.
/// `tests/diagnostics/E0062-nested` and `E0046-nested` pin the wording.
#[test]
fn an_optional_of_an_optional_should_be_peeled_first() {
    let src = with_body("let r = aa?.b");
    let d = diagnostic(&src, Code::MayBeNone, (2, 13, "aa"), &[]);
    assert_eq!(fixes(&src, &d), []);
    let src = with_body("let r = p is q");
    let d = diagnostic(&src, Code::IsNeedsObjects, (2, 13, "p"), &[]);
    assert_eq!(fixes(&src, &d), []);
}

/// `??` groups to the right, so `x ?? None ?? 0` checks `None ?? 0`, whose `None` has no type.
/// The error is at that `None`, and its unsafe fix groups the left side instead.
/// `tests/diagnostics/E0050-coalesce` pins the wording.
#[test]
fn a_none_that_starts_the_right_of_coalesce_should_offer_to_group_the_left() {
    let src = with_body("let r = x ?? None ?? 0");
    let d = diagnostic(&src, Code::CannotInfer, (2, 18, "None"), &[]);
    let edit = |col, text: &str| (2, col, "", text.to_string());
    assert_eq!(
        fixes(&src, &d),
        [(false, vec![edit(13, "("), edit(22, ")")])]
    );
    clean(&applied(&src, &d));
    // The right side of the inner `??` is still checked.
    let src = with_body("let r = x ?? None ?? nothing");
    codes(
        &src,
        &[
            (Code::CannotInfer, (2, 18)),
            (Code::BindingNotFound, (2, 26)),
        ],
    );
}

/// Only a left side that is an optional of an optional, and a `None` that no parenthesis
/// groups, get the fix: the others keep the help to write the type.
#[test]
fn a_none_beside_coalesce_should_keep_the_type_help_when_the_grouping_is_not_the_cause() {
    for (body, expected) in [
        // The right side is written as a group already.
        (
            "let r = x ?? (None ?? 0)",
            vec![(Code::CannotInfer, (2, 19))],
        ),
        // A left side that is not optional.
        (
            "let r = c ?? None ?? 0",
            vec![
                (Code::CoalesceNotOptional, (2, 13)),
                (Code::CannotInfer, (2, 18)),
            ],
        ),
        // One `?` is unwrapped once, so grouping the left side removes no more.
        ("let r = w ?? None ?? 0", vec![(Code::CannotInfer, (2, 18))]),
    ] {
        let src = with_body(body);
        codes(&src, &expected);
        let ds = diags(&src);
        let d = ds
            .iter()
            .find(|d| d.code() == Code::CannotInfer)
            .expect(body);
        assert!(d.help().is_some(), "{body}");
        assert_eq!(fixes(&src, d), [], "{body}");
    }
}

/// A run of `None ??` reports its first `None` only: the fix groups the left side, and the next
/// run finds the next `None`. The rest of the right side is still checked.
#[test]
fn a_run_of_none_beside_coalesce_should_report_only_the_first() {
    let src = "fn g(a: i64???) {\n    let r = a ?? None ?? None ?? 0\n}\nfn main() {}";
    let d = diagnostic(src, Code::CannotInfer, (2, 18, "None"), &[]);
    assert!(d.help().is_none());
    let src = "fn g(a: i64???) {\n    let r = a ?? None ?? None ?? nothing\n}\nfn main() {}";
    codes(
        src,
        &[
            (Code::CannotInfer, (2, 18)),
            (Code::BindingNotFound, (2, 34)),
        ],
    );
    // The run ends at an operand that is not a `None`, and that operand is checked.
    let src = "fn g(a: i64???) {\n    let r = a ?? None ?? nothing ?? 0\n}\nfn main() {}";
    codes(
        src,
        &[
            (Code::CannotInfer, (2, 18)),
            (Code::BindingNotFound, (2, 26)),
        ],
    );
}

/// An operand that is not a `None` ends the run, and a value of the right type is no mistake.
#[test]
fn a_coalesce_whose_right_side_starts_with_a_value_should_check_clean() {
    clean(&with_body("let r = x ?? w ?? 0"));
}

/// An optional is a type argument like any other type, written or inferred.
#[test]
fn an_optional_should_be_a_type_argument() {
    clean(&with_decls("fn f(s: Stack<i64?>) {}"));
    infers(&[
        (
            "fn g() {\n    let x = first<i64?>([])\n}",
            "first",
            &["i64?", "i64??"],
        ),
        (
            "fn g() {\n    let n: i64? = None\n    let x = id(n)\n}",
            "id",
            &["i64?", "i64?"],
        ),
        (
            "fn g() {\n    let x: i64? = id(None)\n}",
            "id",
            &["i64?", "i64?"],
        ),
        // `Some(None)` and `Option.Some(None)` wait for their context, as a generic call does.
        (
            "fn g() {\n    let x: i64?? = id(Some(None))\n}",
            "id",
            &["i64??", "i64??"],
        ),
        (
            "fn g() {\n    let x: i64?? = id(Option.Some(None))\n}",
            "id",
            &["i64??", "i64??"],
        ),
        (
            "fn g() {\n    let x: i64?? = id(Option<i64?>.Some(None))\n}",
            "id",
            &["i64??", "i64??"],
        ),
    ]);
}

/// `Option<T>` is `T?`, and `Some`, `Option.Some`, `Option.None` and `Option<T>.None` build one.
#[test]
fn option_should_spell_an_optional() {
    for (body, text, ty) in [
        ("let r: Option<i64> = 5", "5", "i64?"),
        ("let r: Option<i64?> = None", "None", "i64??"),
        ("let r = Some(5)", "Some(5)", "i64?"),
        ("let r: i64?? = Some(None)", "Some(None)", "i64??"),
        ("let r = Some(Some(5))", "Some(Some(5))", "i64??"),
        ("let r = Some(w)", "Some(w)", "i64??"),
        ("let r = Option.Some(5)", "Option.Some(5)", "i64?"),
        ("let r: i64? = Option.None", "Option.None", "i64?"),
        ("let r = Option<i64>.None", "Option<i64>.None", "i64?"),
        ("let r = Option<i64>.Some(5)", "Option<i64>.Some(5)", "i64?"),
        // The expected payload types the literals inside, as a conversion's does.
        (
            "let r: u8? = Some(if c { 1 } else { 2 })",
            "Some(if c { 1 } else { 2 })",
            "u8?",
        ),
    ] {
        assert_eq!(type_at(&with_body(body), text), ty, "{body}");
    }
    clean("fn f(x: i64?) -> Option<i64> {\n    x\n}\nfn main() {}");
}

/// `src` checked with the embedded prelude, which declares `Option`.
fn with_prelude(src: &str) -> tir::Program {
    let prelude = prelude_program(PRELUDE_SOURCE);
    let (program, ds) = check(&parse_ok(src), &prelude, src, &[]);
    assert!(ds.is_empty(), "{ds:?}");
    program.expect("a program without diagnostics should have a tir")
}

/// `Some(5)` is the conversion of `5`, and the prelude's `Option` makes no enum of its own.
#[test]
fn some_should_lower_as_the_conversion_does() {
    let main =
        |init: &str| format!("fn main() {{\n    let x: i64? = {init}\n    print(x ?? 0)\n}}");
    let converted = with_prelude(&main("5"));
    for init in ["Some(5)", "Option.Some(5)"] {
        let spelled = with_prelude(&main(init));
        assert_eq!(spelled.to_string(), converted.to_string(), "{init}");
        assert!(spelled.enums.iter().all(|e| e.name != "Option"), "{init}");
    }
}

/// `Option.None()` holds nothing: the fix removes the parentheses, and it is unsafe when they hold
/// anything.
#[test]
fn a_none_that_is_called_should_get_the_fix_that_removes_the_parentheses() {
    for (body, parens, safe) in [
        ("let r: i64? = Option.None()", "()", true),
        ("let r: i64? = Option.None(1)", "(1)", false),
        (
            "let r: i64? = Option.None(// 1\n    )",
            "(// 1\n    )",
            false,
        ),
    ] {
        let src = with_body(body);
        let d = diagnostic(&src, Code::VariantForm, (2, 30, parens), &[]);
        let [(is_safe, edits)] = &fixes(&src, &d)[..] else {
            panic!("`{body}` should have one fix");
        };
        assert_eq!((*is_safe, edits.len()), (safe, 1), "{body}");
    }
}

/// An argument that fails still gives `Some` the type it has, so a use of it is checked.
#[test]
fn a_some_whose_argument_fails_should_keep_its_type() {
    codes(
        &with_body("let r: i64 = Some(nothing)"),
        &[(Code::Mismatch, (2, 18)), (Code::BindingNotFound, (2, 23))],
    );
}

/// A wrong use of `Option` or `Some` gets the code of the rule it breaks, at the user's text.
/// `tests/diagnostics/E0050-some` and `E0050-option-some` pin the wording for `Some(None)`.
#[test]
fn a_wrong_use_of_option_or_some_should_be_reported() {
    for (body, code, at) in [
        (
            "let r = Some(1, 2)",
            Code::ArgumentCount,
            (2, 13, "Some(1, 2)"),
        ),
        ("let r = Some", Code::VariantForm, (2, 13, "Some")),
        ("let r = x ?? Some(None)", Code::Mismatch, (2, 23, "None")),
        (
            "let r: Option<i64, i64> = None",
            Code::TypeArguments,
            (2, 18, "<i64, i64>"),
        ),
        (
            "let r: Option = None",
            Code::TypeArguments,
            (2, 12, "Option"),
        ),
        ("let r = Some(None)", Code::CannotInfer, (2, 13, "Some")),
        (
            "let r = Option.Some(None)",
            Code::CannotInfer,
            (2, 13, "Option.Some"),
        ),
        ("let r = Option.Some", Code::VariantForm, (2, 20, "Some")),
        // `Option.Some` is no form of `None`, so `is` does not test it as one.
        (
            "let r = Option.Some is a",
            Code::VariantForm,
            (2, 20, "Some"),
        ),
        ("let r = Option.map", Code::UnknownVariant, (2, 20, "map")),
        (
            "let r = Option.map(1)",
            Code::UnknownVariant,
            (2, 20, "map"),
        ),
        (
            "let r = Option<i64, i64>.None",
            Code::TypeArguments,
            (2, 19, "<i64, i64>"),
        ),
        // The type arguments of a variant go on `Option`, as on an enum.
        (
            "let r = Option.Some<Missing>(5)",
            Code::TypeArguments,
            (2, 24, "<Missing>"),
        ),
        (
            "let r = Option<i64>.Some<Missing>(5)",
            Code::TypeArguments,
            (2, 29, "<Missing>"),
        ),
        (
            "let r = Option.None<i64>()",
            Code::TypeArguments,
            (2, 24, "<i64>"),
        ),
        // A written type argument is not overridden by the expected type.
        (
            "let r: u8? = Some<i64>(5)",
            Code::Mismatch,
            (2, 18, "Some<i64>(5)"),
        ),
        // The call fails with its label, so it gives no value to mismatch.
        (
            "let r: i64 = Some(a: 1)",
            Code::LabeledArgument,
            (2, 23, "a"),
        ),
    ] {
        diagnostic(&with_body(body), code, at, &[]);
    }
}

/// A call of a name that is not `Some` and names nothing waits for no context: it is the one
/// error, with no failed inference after it.
#[test]
fn an_unknown_call_should_not_wait_for_context_in_a_generic_call() {
    let src = with_generics("fn g() {\n    let x = id(unknown(None))\n}");
    codes(&src, &[(Code::FunctionNotFound, (2, 16))]);
}

/// `body` on lines 2 on of `fn g` over `o: i64?`, `s: Shape?`, `oo: i64??`, `h: Holder`,
/// `k: Num`, `p: Pile` and `l: Light??`, then the enums `Shape`, `Holder`, whose `V` holds a
/// `Shape?`, `Num`, whose `N` holds an `i64?`, and `Light`, of `On` and `Off`, the class `Pile`,
/// whose `pop` gives an `i64?`, and an empty `main`.
fn with_shapes(body: &str) -> String {
    format!(
        "fn g(o: i64?, s: Shape?, oo: i64??, h: Holder, k: Num, p: Pile, l: Light??) {{\n    {body}\n}}\nenum Shape {{\n    Circle(f64)\n    Square(f64)\n}}\nenum Holder {{\n    V(Shape?)\n}}\nenum Num {{\n    N(i64?)\n}}\nenum Light {{\n    On\n    Off\n}}\nclass Pile {{\n    mut items: [i64]\n    fn pop(self) -> i64? {{ None }}\n}}\nfn main() {{}}"
    )
}

/// At an optional position, `Some(p)` and `None` match the optional itself, a variant, literal
/// or range pattern matches the value inside, an `|` takes the rule for each alternative, and a
/// name or `_` covers the whole optional; only the top of an `if let` or `while let` unwraps.
#[test]
fn a_pattern_on_an_optional_should_match_the_optional_or_the_value_inside() {
    for (body, text, ty) in [
        (
            "let r = match o {\n        Some(0) => 1\n        Some(n) => n\n        None => 0\n    }",
            "n",
            "i64",
        ),
        (
            "let r = match o {\n        0 => 1\n        None => 2\n        _ => 3\n    }",
            "2",
            "i64",
        ),
        (
            "let r = match s {\n        Circle(r) => r\n        Square(w) => w\n        None => 0.0\n    }",
            "w",
            "f64",
        ),
        (
            "let r = match oo {\n        Some(Some(n)) => n\n        Some(None) => 1\n        None => 0\n    }",
            "n",
            "i64",
        ),
        ("let r = match o {\n        _ => 0\n    }", "0", "i64"),
        (
            "let r = match o {\n        Some(n) => n\n        x => x ?? 0\n    }",
            "x",
            "i64?",
        ),
        (
            "if let Circle(r) = s {\n        let t = r\n    }",
            "r",
            "f64",
        ),
        ("if let Some(n) = o {\n        let t = n\n    }", "n", "i64"),
        ("if let y = o {\n        let t = y\n    }", "y", "i64"),
        (
            "while let Some(n) = p.pop() {\n        let t = n\n    }",
            "n",
            "i64",
        ),
        (
            "let r = match h {\n        V(None) => 0\n        V(Some(q)) => {\n            let t = q\n            1\n        }\n    }",
            "q",
            "Shape",
        ),
        (
            "let r = match h {\n        V(Circle(r)) => r\n        _ => 0.0\n    }",
            "r",
            "f64",
        ),
        ("let r = match h {\n        V(z) => z\n    }", "z", "Shape?"),
        (
            "let r = match o {\n        0..=9 => 1\n        10 | 11 => 2\n        Some(_) => 3\n        None => 4\n    }",
            "4",
            "i64",
        ),
        (
            "let r = match o {\n        Some(0) | None => 1\n        _ => 2\n    }",
            "1",
            "i64",
        ),
        (
            "let r = match k {\n        N(1..=3 | None) => 1\n        _ => 2\n    }",
            "1",
            "i64",
        ),
        (
            "let r = match o {\n        Option.Some(m) => m\n        None => 0\n    }",
            "m",
            "i64",
        ),
        (
            "let r = match l {\n        On => 1\n        Off => 2\n        Some(None) => 3\n        None => 4\n    }",
            "3",
            "i64",
        ),
    ] {
        assert_eq!(type_at(&with_shapes(body), text), ty, "{body}");
    }
}

/// `Option.None` is the pattern `None`, at the top and under a `Some`; it covers the `None` of a
/// `match`.
#[test]
fn an_option_none_pattern_should_mean_none() {
    contains(
        &with_shapes(
            "let r = match oo {\n        Some(Some(n)) => n\n        Some(Option.None) => 1\n        Option.None => 0\n    }",
        ),
        &[
            "(arm (Some (Some n#7)) (block n#7))",
            "(arm (Some None) (block 1))",
            "(arm None (block 0))",
        ],
    );
}

/// The `tir` prints a `Some` pattern with its inner pattern, the implicit `Some` too.
#[test]
fn a_some_pattern_should_print_with_its_inner_pattern() {
    contains(
        &with_shapes(
            "let r = match s {\n        Circle(r) => r\n        Some(q) => 0.0\n        None => 0.0\n    }",
        ),
        &[
            "(arm (Some (Circle r#7)) (block r#7))",
            "(arm (Some q#8) (block 0.0))",
            "(arm None (block 0.0))",
        ],
    );
}

/// A `match` on an optional that leaves a value out is not exhaustive: `None`, a value, a
/// variant of the payload, or the `None` of a middle layer. The `witnesses_should_name...` table
/// of `check::exhaust` pins what each one misses.
#[test]
fn a_match_on_an_optional_should_cover_none_and_every_value() {
    for body in [
        "let r = match o {\n        Some(n) => n\n    }",
        "let r = match o {\n        None => 0\n    }",
        "let r = match s {\n        Circle(r) => 1\n        None => 0\n    }",
        "let r = match oo {\n        Some(Some(n)) => n\n        None => 0\n    }",
    ] {
        codes(&with_shapes(body), &[(Code::NonExhaustive, (2, 13))]);
    }
}

/// The branches of an `if`, an `if let` and a `match` rank as the elements of an array do. A
/// branch that waits for a type and is no optional by its syntax takes the type of the others as
/// it is. A branch whose value is a `None` through a block or `dbg` makes the type optional.
#[test]
fn a_branch_that_waits_should_take_the_type_of_the_others() {
    super::with_prelude();
    let decls = "fn id<T>(x: T) -> T { x }\n";
    for statement in [
        "let _a = if c { id([]) } else { [1.0] }",
        "let _a = if c { ({ None }) } else { 1 }",
        "let _a = if c { 1 } else { ({ None }) }",
        "let _a = if let v = y { ({ None }) } else { 1 }",
        "let _a = if let v = y { ({ 1 }) } else { x }",
        "let _a = match c {\n        true => dbg(None)\n        false => 1\n    }",
        "let _a = match c {\n        true => { 1 }\n        false => x\n    }",
        // A value that is no `None` keeps the optional of a branch that is.
        "let _a = if c { x } else { y }",
        "let _a = if c { x } else { 1 }",
        "let _a = if c { x } else { if c { 1 } else { None } }",
        "let _a = if c { [1] } else { [] }",
        // A call that makes an optional by itself keeps it, as it did.
        "let _a = if c { x } else { id(None) }",
        "let _a = if c { id(None) } else { x }",
        "let _a = match c {\n        true => x\n        false => id(None)\n    }",
    ] {
        let src = format!(
            "{decls}fn main() {{\n    let c = true\n    let x: u8 = 3\n    let y: u8? = 4\n    {statement}\n}}"
        );
        clean_row(statement, &src);
    }
}

/// A branch that waits and is no optional by its syntax expects the type of the others as it is,
/// so it is reported for what it is, once.
#[test]
fn a_waiting_branch_that_is_no_optional_should_be_reported_for_its_type() {
    for (statement, text) in [
        ("let _a = if c { [] } else { 1 }", "[]"),
        ("let _a = if c { 1 } else { [] }", "[]"),
        (
            "let _a = match c {\n        true => []\n        false => 1\n    }",
            "[]",
        ),
    ] {
        let src = format!("fn main() {{\n    let c = true\n    {statement}\n}}");
        let start = src
            .rfind(statement)
            .expect("the program should hold the statement");
        let at = line_col(
            &src,
            start
                + statement
                    .find(text)
                    .expect("the statement should hold the text"),
        );
        codes_row(statement, &src, &[(Code::Mismatch, at)]);
    }
}
