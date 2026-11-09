//! The work a path does to find what its first name stands for.

use super::*;

/// How many times `binding` walks the open scopes while `src` is checked.
fn walks(src: &str) -> usize {
    BINDING_STEPS.set(0);
    diags(src);
    BINDING_STEPS.get()
}

/// Walking the scopes for a local costs the depth of the scopes, so a path that no local can
/// change needs no walk: a Known enum wins over a local, `Option` has no item, and a local cannot
/// hide an alias of an enum.
#[test]
fn a_path_no_local_can_change_should_not_walk_the_scopes() {
    let rows = [
        (
            "a known enum",
            "enum E { A }\nfn main() {\n    let _a = E.A\n}",
        ),
        ("Option", "fn main() {\n    let _a: i64? = Option.None\n}"),
        (
            "an alias of an enum",
            "enum E { A }\ntype N = E\nfn main() {\n    let _a = N.A\n}",
        ),
    ];
    let walking: Vec<_> = rows
        .iter()
        .filter(|(_, src)| walks(src) != 0)
        .map(|(row, _)| *row)
        .collect();
    assert!(
        walking.is_empty(),
        "these paths walk the scopes: {walking:?}"
    );
    // A control: a path whose first name is a local needs the walk, so the counter works.
    assert!(walks("fn f(N: i64) {\n    let _a = N.x\n}\nfn main() {}") > 0);
}
