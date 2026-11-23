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
