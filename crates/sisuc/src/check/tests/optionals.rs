//! Optionals of optionals: each operation takes one layer at a time.

use super::generics::{infers, with_decls};
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
/// to the other. `tests/diagnostics/E0032-nested-optional` pins the wording.
#[test]
fn a_value_should_convert_through_one_layer_only() {
    for (body, at) in [
        ("let r: i64?? = 5", (2, 20, "5")),
        ("let r = x ?? \"a\"", (2, 18, "\"a\"")),
        ("let r = x == 5", (2, 18, "5")),
    ] {
        diagnostic(&with_body(body), Code::Mismatch, at, &[]);
    }
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
            "fn g() {\n    let x: i64? = id(5)\n}",
            "id",
            &["i64?", "i64?"],
        ),
        // `Some(None)` waits for its context, as a call of a generic function does.
        (
            "fn g() {\n    let x: i64?? = id(Some(None))\n}",
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

/// A wrong use of `Option` or `Some` gets the code of the rule it breaks, at the user's text.
/// `tests/diagnostics/E0050-some` pins the wording of the last.
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
    ] {
        diagnostic(&with_body(body), code, at, &[]);
    }
}
