//! `let ... else`: the names it binds, its tests and the loops its `else` block reaches.

use super::*;

/// The typed tree of `src`, which must check clean.
fn program(src: &str) -> tir::Program {
    let (program, ds) = checked(src);
    assert!(ds.is_empty(), "{ds:?}");
    program.expect("a clean program should have a typed tree")
}

fn function<'a>(program: &'a tir::Program, name: &str) -> &'a tir::Function {
    (program.functions.iter().find(|f| f.name == name))
        .expect("the program should hold the function")
}

/// The first statement of `function`, which must be a `let ... else`.
fn let_else(function: &tir::Function) -> (&tir::Expr, &tir::LetElseTest, &tir::Block) {
    let Some(tir::Stmt::LetElse {
        scrutinee,
        test,
        else_block,
    }) = function.body.stmts.first()
    else {
        panic!("the first statement should be a `let ... else`");
    };
    (scrutinee, test, else_block)
}

/// Whether `function` reads `local` anywhere.
fn reads(function: &tir::Function, local: tir::LocalId) -> bool {
    let mut all = Vec::new();
    function.body.exprs(&mut all);
    all.iter()
        .any(|e| matches!(e.kind, tir::ExprKind::Local(l) if l == local))
}

const SHAPE: &str = "enum Shape {\n    Circle(f64)\n    Rect(f64, f64)\n    Square(f64)\n}\n";

/// GIVEN a function `first(xs: [i64?]) -> i64` whose body is `let v = xs[0] else { return -1 }`
/// and then `v * 2`
/// WHEN `sisu check` runs on it
/// THEN the typed tree binds `v` with `LetElseTest::Some` and the block's value reads it
#[test]
fn let_else_names_should_be_in_scope_after_the_statement() {
    let program = program(
        "fn first(xs: [i64?]) -> i64 {\n    let v = xs[0] else { return -1 }\n    v * 2\n}\nfn main() {\n    print(first([Some(4)]))\n}",
    );
    let first = function(&program, "first");
    let (_, test, _) = let_else(first);
    let tir::LetElseTest::Some { bind } = test else {
        panic!("an optional should give `LetElseTest::Some`");
    };
    assert_eq!(first.locals[bind.0].name.as_deref(), Some("v"));
    assert!(reads(first, *bind));
}

/// GIVEN `enum Shape { Circle(f64); Rect(f64, f64) }` and a function `area(s: Shape) -> f64`
/// whose body is `let Circle(r) = s else { return 0.0 }` and then `r * r`
/// WHEN `sisu check` runs on it
/// THEN the typed tree tests the pattern with `LetElseTest::Match`, which binds `r`
#[test]
fn let_else_on_an_enum_should_test_the_pattern() {
    let src = format!(
        "{SHAPE}fn area(s: Shape) -> f64 {{\n    let Circle(r) = s else {{ return 0.0 }}\n    r * r\n}}\nfn main() {{\n    print(area(Shape.Circle(2.0)))\n    print(area(Shape.Rect(1.0, 1.0)))\n    print(area(Shape.Square(1.0)))\n}}"
    );
    let program = program(&src);
    let area = function(&program, "area");
    let (_, test, _) = let_else(area);
    let tir::LetElseTest::Match {
        pattern, bindings, ..
    } = test
    else {
        panic!("an enum should give `LetElseTest::Match`");
    };
    assert!(matches!(pattern.kind, tir::PatternKind::Variant { .. }));
    let [bind] = bindings.as_slice() else {
        panic!("the pattern should bind one name");
    };
    assert_eq!(area.locals[bind.0].name.as_deref(), Some("r"));
    assert!(reads(area, *bind));
}

/// GIVEN the `Shape` with a third variant `Square(f64)`
/// AND a function whose body is `let Circle(r) | Square(r) = s else { return 0.0 }` and then `r`
/// WHEN `sisu check` runs on it
/// THEN the pattern binds one `r` in both alternatives
#[test]
fn let_else_alternatives_should_bind_in_each() {
    let src = format!(
        "{SHAPE}fn side(s: Shape) -> f64 {{\n    let Circle(r) | Square(r) = s else {{ return 0.0 }}\n    r\n}}\nfn main() {{\n    print(side(Shape.Square(3.0)))\n    print(side(Shape.Rect(1.0, 1.0)))\n    print(side(Shape.Circle(1.0)))\n}}"
    );
    let program = program(&src);
    let side = function(&program, "side");
    let (_, test, _) = let_else(side);
    let tir::LetElseTest::Match {
        pattern, bindings, ..
    } = test
    else {
        panic!("an enum should give `LetElseTest::Match`");
    };
    let tir::PatternKind::Or(alternatives) = &pattern.kind else {
        panic!("the pattern should keep its alternatives");
    };
    // Each alternative binds the local that the statement binds, which the lines after it read.
    let [bind] = bindings.as_slice() else {
        panic!("the pattern should bind one name");
    };
    let bound: Vec<_> = alternatives
        .iter()
        .map(|alternative| match &alternative.kind {
            tir::PatternKind::Variant { fields, .. } => match fields.as_slice() {
                [
                    tir::Pattern {
                        kind: tir::PatternKind::Bind(local),
                        ..
                    },
                ] => Some(*local),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(bound, [Some(*bind), Some(*bind)]);
}

/// GIVEN `outer: for i in 0..3` with an inner `for o in [Some(i), None]` whose body is
/// `let v = o else { continue outer }` and then `print(v)`
/// WHEN `sisu check` runs on it
/// THEN the program checks clean and the `continue` names the outer loop
#[test]
fn let_else_block_should_reach_the_loops_around_it() {
    let program = program(
        "fn main() {\n    outer: for i in 0..3 {\n        for o in [Some(i), None] {\n            let v = o else { continue outer }\n            print(v)\n        }\n    }\n}",
    );
    let mut all = Vec::new();
    function(&program, "main").body.exprs(&mut all);
    let depths: Vec<usize> = all
        .iter()
        .filter_map(|e| match e.kind {
            tir::ExprKind::Continue { depth } => Some(depth),
            _ => None,
        })
        .collect();
    // The inner `for` is the loop nearest to the `continue`; its outer loop is one out.
    assert_eq!(depths, [1]);
}

/// GIVEN a `main` with `let o: i64? = Some(1)`, then `let mut n = o else { return }`, then
/// `n = n + 1` and `print(n)`
/// WHEN `sisu check` runs on it
/// THEN `n` is a `let mut` local and the program checks clean
#[test]
fn let_mut_else_should_declare_a_variable() {
    let program = program(
        "fn main() {\n    let o: i64? = Some(1)\n    let mut n = o else { return }\n    n = n + 1\n    print(n)\n}",
    );
    let main = function(&program, "main");
    let tir::Stmt::LetElse { test, .. } = &main.body.stmts[1] else {
        panic!("the second statement should be a `let ... else`");
    };
    let tir::LetElseTest::Some { bind } = test else {
        panic!("an optional should give `LetElseTest::Some`");
    };
    assert!(main.locals[bind.0].mutable);
}

/// `let mut` keeps its keyword, so W0002 judges the name, and a name that nothing changes gets it.
#[test]
fn let_mut_else_should_warn_when_nothing_changes_the_name() {
    judge_unused();
    let ds = diags(
        "fn main() {\n    let o: i64? = Some(1)\n    let mut n = o else { return }\n    print(n)\n}",
    );
    let codes: Vec<_> = ds.iter().map(Diagnostic::code).collect();
    assert_eq!(codes, [Code::NeverReassigned]);
}

/// A name that the pattern binds and nothing reads gets W0014, as a `let` name does.
#[test]
fn let_else_should_warn_when_nothing_reads_the_name() {
    judge_unused();
    let ds = diags("fn main() {\n    let o: i64? = Some(1)\n    let v = o else { return }\n}");
    let codes: Vec<_> = ds.iter().map(Diagnostic::code).collect();
    assert_eq!(codes, [Code::UnusedVariable]);
}

/// A name has nothing to compare, so E0051 offers `==` only for a pattern that is a value.
#[test]
fn let_else_on_a_plain_value_should_offer_a_comparison_only_for_a_literal_pattern() {
    let src = |line: &str| format!("fn main() {{\n    let z = 5\n    {line}\n}}");
    for (line, help) in [
        ("let w = z else { return }", false),
        ("let mut n = z else { return }", false),
        ("if let x = z {}", false),
        ("let 3 = z else { return }", true),
    ] {
        let ds = diags(&src(line));
        let d = (ds.iter().find(|d| d.code() == Code::UnwrapNeedsOptional))
            .unwrap_or_else(|| panic!("{line}: {ds:?}"));
        assert_eq!(d.help().is_some(), help, "{line}");
    }
}
