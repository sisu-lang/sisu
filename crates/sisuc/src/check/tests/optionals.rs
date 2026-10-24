//! Optionals of optionals: each operation takes one layer at a time.

use super::generics::{infers, with_decls};
use super::*;

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
    ]);
}
