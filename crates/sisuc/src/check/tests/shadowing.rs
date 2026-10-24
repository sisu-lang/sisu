//! Same-scope shadowing: a `let` or `let mut` may redeclare a name, and the new binding replaces
//! the old one from its declaration onwards.

use super::*;

#[test]
fn rhs_sees_old_binding() {
    contains(
        "fn main() {\n    let n = 1\n    let n = n + 1\n    print(n)\n}",
        &["(let n#1 (+ n#0 1))", "(print n#1)"],
    );
}

#[test]
fn type_can_change() {
    clean("fn main() {\n    let line = [1]\n    let line = line.len()\n    print(line)\n}");
}

#[test]
fn let_mut_shadows_let() {
    clean("fn main() {\n    let x = 1\n    let mut x = 2\n    x = 3\n    print(x)\n}");
}

#[test]
fn if_let_binding_shadowed() {
    for head in ["if let", "while let"] {
        clean(&format!(
            "fn f(m: i64?) {{\n    {head} x = m {{\n        let x = 1\n        print(x)\n    }}\n}}\nfn main() {{}}"
        ));
    }
}

/// A shadowed `let mut` can no longer be reassigned, so it warns at once, unless it was
/// reassigned before the shadowing `let`. The shadowing `let` reads it, so W0014 does not
/// take the place of W0002.
#[test]
fn shadowed_variable_should_warn_unless_reassigned_first() {
    codes(
        "fn main() {\n    let mut a = 0\n    let a = a + 1\n    print(a)\n}",
        &[(Code::NeverReassigned, (2, 13))],
    );
    clean("fn main() {\n    let mut a = 0\n    a = 1\n    let a = a + 1\n    print(a)\n}");
}

/// The error's secondary span is the shadowing `let`'s name, and its safe fix makes that
/// `let` a `let mut`, which leaves only the shadowed binding's warning.
#[test]
fn redeclared_label_same_scope() {
    let src = "fn main() {\n    let mut total = 0\n    let total = total\n    total += 1\n}";
    codes(
        src,
        &[
            (Code::NeverReassigned, (2, 13)),
            (Code::AssignToImmutable, (4, 5)),
        ],
    );
    let ds = diags(src);
    let d = ds
        .iter()
        .find(|d| d.code() == Code::AssignToImmutable)
        .expect("the assignment should be reported");
    let secondary: Vec<_> = d.secondary().iter().map(|(s, _)| at(src, *s)).collect();
    assert_eq!(secondary, [(3, 9)]);
    assert_eq!(
        fixes(src, d),
        [(true, vec![(3, 9, "", "mut ".to_string())])]
    );
    codes(&applied(src, d), &[(Code::NeverReassigned, (2, 13))]);
}

#[test]
fn redeclared_label_outer_scope() {
    let src = "fn main() {\n    let mut total = 0\n    total = 1\n    if true {\n        let total = total\n        total += 1\n    }\n}";
    let d = diagnostic(
        src,
        Code::AssignToImmutable,
        (6, 9, "total += 1"),
        &[(5, 13)],
    );
    assert_eq!(
        fixes(src, &d),
        [(true, vec![(5, 13, "", "mut ".to_string())])]
    );
    clean(&applied(src, &d));
}
