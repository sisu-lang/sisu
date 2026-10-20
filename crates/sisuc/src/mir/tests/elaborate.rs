//! Elaboration: each scope release becomes nothing, a plain release or stays conditional, by
//! the set of states that reaches it.

use super::*;
use crate::mir::certify::{Rule, Violation, certify};
use crate::mir::elaborate::elaborate;

const KEEP: &str = "fn keep(p: [i64]) {\n    let kept = p\n}\nfn name() -> string { \"n\" }\n";

/// Moved on every path: the scope release is removed.
#[test]
fn release_of_a_moved_local_is_removed() {
    let printed = built(&format!("{KEEP}fn f(xs: [i64]) {{\n    keep(xs)\n}}"));
    let f = function(&printed, "f");
    assert!(f.contains("call keep(move xs#0)"), "{f}");
    assert!(!f.contains("release"), "{f}");
}

/// Owned on every path out of the loop: a plain release.
#[test]
fn release_of_an_owned_local_is_static() {
    let printed = built(&format!(
        "{KEEP}fn f(c: bool) {{\n    let s = name()\n    while c {{\n        print(s.len())\n    }}\n}}"
    ));
    let f = function(&printed, "f");
    assert!(f.contains("release s#1"), "{f}");
    assert!(!f.contains("release? s#1"), "{f}");
}

/// Moved on one path, owned on the other: still conditional.
#[test]
fn release_after_a_move_on_one_path_stays_conditional() {
    let printed = built(&format!(
        "{KEEP}fn f(xs: [i64], c: bool) {{\n    if c {{\n        keep(xs)\n    }}\n}}"
    ));
    assert_eq!(function(&printed, "f").matches("release? xs#0").count(), 1);
}

/// Review Focus 2: each exit edge is classified on its own.
#[test]
fn each_exit_edge_is_elaborated_on_its_own() {
    let printed = built(&format!(
        "{KEEP}fn f(xs: [i64], c: bool) {{\n    while true {{\n        if c {{\n            keep(xs)\n            return\n        }}\n        break\n    }}\n}}"
    ));
    let f = function(&printed, "f");
    // The `return` edge follows the move and releases nothing; the `break` edge owns `xs`.
    assert!(f.contains("call keep(move xs#0)"), "{f}");
    assert_eq!(f.matches("release xs#0").count(), 1, "{f}");
    assert!(!f.contains("release? xs#0"), "{f}");
}

/// Review Focus 5: the scope release sees {owned}, but the region fails later, at the second
/// release. A failing region contributes nothing, so the scope release stays for the certifier,
/// and `t`, checked after `s`, is elaborated from its own observations only.
#[test]
fn release_in_a_failing_region_is_kept() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    let t = b.local("t", Type::STRING);
    b.block(
        vec![
            fresh(s),
            Statement::ScopeRelease(s),
            Statement::Release(s),
            fresh(t),
            Statement::ScopeRelease(t),
        ],
        ret(),
    );
    let mut f = b.finish();
    elaborate(&mut f);
    let stmts = &f.blocks[0].stmts;
    // `s`'s scope release and the hand-written release stay, in order; `t`'s becomes a release.
    assert!(
        matches!(
            stmts.as_slice(),
            [_, Statement::ScopeRelease(kept), Statement::Release(written), _, Statement::Release(made)]
                if *kept == s && *written == s && *made == t
        ),
        "{stmts:?}"
    );
    assert_eq!(
        certify(&f),
        Err(Violation {
            block: BlockId(0),
            local: s,
            rule: Rule::Consume,
        })
    );
}
