//! A `let` without a type keeps the inference variables of its value open until the first use of
//! its local. That use closes them as the `let` closes them on `main`.

use super::joins::with_params;
use super::*;
use crate::check::stmt::BlockSoFar;

/// The open roots of the table in the type of the local `name`, as its binding holds it.
fn open_roots(c: &mut Checker<'_>, name: &str) -> usize {
    let ty = c
        .lookup(Symbol::intern(name))
        .expect("the local should be in scope")
        .ty;
    ty.map_or(0, |ty| c.table.open_roots(ty).len())
}

#[test]
fn let_without_a_type_should_keep_its_roots_open() {
    let src = "fn t() {\n    let x = 1\n    let y = None\n    let z = []\n    print(x)\n    print(y)\n    print(z)\n}\n";
    let block = {
        let full = format!("{src}fn probe() {{}}\nfn main() {{}}\n");
        let program = parse_ok(&full);
        let Some(ast::Item::Function(t)) = program.items.first() else {
            panic!("the first item should be `t`");
        };
        t.body.clone()
    };
    with_params(src, "", |c, _| {
        c.body.scopes.push(FxHashMap::default());
        let mut so_far = BlockSoFar::default();
        // GIVEN the three `let`s without a type, checked
        c.block_statements(&block, Expectation::None, &mut so_far, 3);
        // THEN before the first read, each local's type holds an open root of the table
        for name in ["x", "y", "z"] {
            assert_ne!(open_roots(c, name), 0, "`{name}` before its first read");
        }
        // WHEN each local is read
        c.block_statements(&block, Expectation::None, &mut so_far, 6);
        // THEN its type holds none
        for name in ["x", "y", "z"] {
            assert_eq!(open_roots(c, name), 0, "`{name}` after its first read");
        }
    });
}

#[test]
fn second_read_should_see_the_closed_type() {
    // GIVEN a local whose `let` gives an integer, read once and then read under another type
    let rows = [
        (
            "a read that wants no type",
            "fn main() {\n    let n = 7\n    print(n)\n    let _b: u8 = n\n}\n",
            (4, 18),
        ),
        (
            "an integer division",
            "fn main() {\n    let n = 7\n    print(n / 2)\n    let _f: f64 = n\n}\n",
            (4, 19),
        ),
    ];
    for (row, src, at) in rows {
        // WHEN it is checked
        // THEN the second read sees `i64`, as on `main`: E0032 at that read
        codes_row(row, src, &[(Code::Mismatch, at)]);
    }
}

#[test]
fn local_that_no_type_reaches_should_report_as_on_main() {
    // GIVEN a `None` or a `[]` that a `let` without a type keeps
    let rows = [
        (
            "a local that nothing reads",
            "fn main() {\n    let _x = None\n}\n",
            vec![(Code::CannotInfer, (2, 14))],
        ),
        (
            "a local pushed twice, with two types",
            "fn main() {\n    let mut pushed = []\n    pushed.push('a')\n    pushed.push(\"b\")\n    print(pushed.len())\n}\n",
            vec![(Code::CannotInfer, (2, 22))],
        ),
    ];
    for (row, src, expected) in rows {
        // WHEN it is checked
        // THEN it gets the E0050 of `main` at its literal, and nothing else
        codes_row(row, src, &expected);
    }
}

#[test]
fn first_write_should_see_the_closed_type() {
    // GIVEN a local whose `let` gives an integer, first written with a `u8`, whole and at an
    // element
    let rows = [
        (
            "a write of the local",
            "fn main() {\n    let b: u8 = 3\n    let mut w = 1\n    w = b\n    print(w)\n}\n",
            (4, 9),
        ),
        (
            "a write with the local at its root",
            "fn main() {\n    let b: u8 = 3\n    let mut a = [0; 3]\n    a[0] = b\n    print(a)\n}\n",
            (4, 12),
        ),
    ];
    for (row, src, at) in rows {
        // WHEN it is checked
        // THEN the write sees `i64`, as on `main`: E0032 at the written value
        codes_row(row, src, &[(Code::Mismatch, at)]);
    }
}

#[test]
fn error_window_before_the_first_use_should_see_the_error_of_main() {
    // GIVEN an E0050 that main writes at its `let`, before a check that reads the error count
    let rows = [
        (
            "an unread `None` in a `while` body: no W0011",
            "fn main() {\n    let i = 0\n    while i < 10 {\n        let _x = None\n    }\n}\n",
            (4, 18),
        ),
        (
            "an unread `[]` in a `while` body: no W0011",
            "fn main() {\n    let i = 0\n    while i < 10 {\n        let _x = []\n    }\n}\n",
            (4, 18),
        ),
        (
            "a `None` before the `get` of a string: no E0032",
            "fn main() {\n    let w = None\n    let s = \"abc\"\n    print(s.get(None))\n}\n",
            (2, 13),
        ),
        (
            "a `[]` before the `get` of a string: no E0032",
            "fn main() {\n    let w = []\n    let s = \"abc\"\n    print(s.get([]))\n}\n",
            (2, 13),
        ),
        (
            "a fill of `None` before the `get` of a string: no E0032",
            "fn main() {\n    let w = [None; 2]\n    let s = \"abc\"\n    print(s.get(None))\n}\n",
            (2, 14),
        ),
        (
            "a `None` in the argument of the `get` of a string: no E0032",
            "fn main() {\n    let s = \"abc\"\n    print(s.get({\n        let _z = None\n        None\n    }))\n}\n",
            (4, 18),
        ),
    ];
    for (row, src, at) in rows {
        // WHEN it is checked
        // THEN the check sees the error, as on `main`: the E0050 is the only diagnostic
        codes_row(row, src, &[(Code::CannotInfer, at)]);
    }
}

#[test]
fn coalesce_default_with_an_open_let_should_give_no_fix() {
    // GIVEN a `??` on a value that is not optional, whose default holds a `let` of a `None`
    let src = "fn main() {\n    let x = 1\n    print(x ?? {\n        let _z = None\n        2\n    })\n}\n";
    // WHEN it is checked
    codes_row(
        "",
        src,
        &[
            (Code::CoalesceNotOptional, (3, 11)),
            (Code::CannotInfer, (4, 18)),
        ],
    );
    // THEN the E0048 has no fix, as on `main`: an error in the default holds it back
    let ds = diags(src);
    assert!(ds[0].fixes().is_empty(), "{:?}", ds[0]);
}

#[test]
fn is_none_on_an_uninferred_local_should_keep_later_use_help() {
    // GIVEN an uninferred `None` local first read in an `is None` test, then assigned `Some(n)`
    let src = "fn main() {\n    let mut best = None\n    for n in [3, 1, 2] {\n        if best is None {\n            best = Some(n)\n        }\n    }\n    print(best)\n}\n";
    // WHEN it is checked
    codes_row(
        "is6",
        src,
        &[(Code::CannotInfer, (2, 20)), (Code::IsTestsNone, (4, 12))],
    );
    // THEN the E0050 keeps the `i64?` type that the later write gives
    let ds = diags(src);
    let expected: Diagnostic = DiagnosticKind::CannotInfer {
        literal: Literal::None,
        binding: Some("mut best".to_string()),
        ty: Some("i64?".to_string()),
        at: ds[0].span(),
        regroup: None,
    }
    .into();
    assert_eq!(ds[0], expected);
}

#[test]
fn generic_call_with_uninferred_let_in_branch_should_report_once() {
    // GIVEN a generic call whose branch argument holds `let acc = []`
    let src = "fn first<T>(xs: [T], d: T) -> T { d }\nfn main() {\n    let c = true\n    print(first(if c {\n        let acc = []\n        acc\n    } else {\n        []\n    }, \"x\"))\n}\n";
    // WHEN it is checked
    // THEN it reports one E0050 at the `[]`, not two
    codes_row("d2", src, &[(Code::CannotInfer, (5, 19))]);
}

#[test]
fn unread_uninferred_let_in_some_branch_should_suppress_print_warning() {
    // GIVEN `Some` around a branch that holds an unread `let z = []`
    let src = "fn main() {\n    let c = true\n    let v = Some(if c {\n        let z = []\n        1\n    } else {\n        2\n    })\n    print(v)\n}\n";
    // WHEN it is checked
    // THEN the failed `let` poisons the branch, so `print(v)` gives no W0015
    codes_row("a3", src, &[(Code::CannotInfer, (4, 17))]);
}

#[test]
fn unread_uninferred_let_in_branch_should_poison_generic_call() {
    // GIVEN a generic call whose branch holds an unread `let z = None`
    let src = "fn pick<T>(a: T, b: T) -> T { a }\nfn main() {\n    let r: i64 = pick(if true { let z = None\n        1 } else { \"b\" }, 1)\n}\n";
    // WHEN it is checked
    // THEN the failed `let` poisons the branch, so the call gives no E0032
    codes_row("s6", src, &[(Code::CannotInfer, (3, 41))]);
}

#[test]
fn array_fill_of_none_with_bad_count_should_report_none_first() {
    // GIVEN an array fill of `None` whose count is a string
    let src = "fn main() {\n    let w = [None; \"a\"]\n    print(1)\n}\n";
    // WHEN it is checked
    // THEN it reports E0050 at `None` before E0032 at the count
    codes_row(
        "f1",
        src,
        &[(Code::CannotInfer, (2, 14)), (Code::Mismatch, (2, 20))],
    );
}

#[test]
fn array_fill_of_empty_array_with_bad_count_should_report_array_first() {
    // GIVEN an array fill of `[]` whose count is a boolean
    let src = "fn main() {\n    let w = [[]; true]\n}\n";
    // WHEN it is checked
    // THEN it reports E0050 at `[]` before E0032 at the count
    codes_row(
        "f2",
        src,
        &[(Code::CannotInfer, (2, 14)), (Code::Mismatch, (2, 18))],
    );
}
