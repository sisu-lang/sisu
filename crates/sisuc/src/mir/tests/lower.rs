use rustc_hash::{FxHashMap, FxHashSet};

use super::*;
use crate::mir::lower::block_mentions;

/// Calls run in source order, and an `i64` is returned by a read, not moved.
#[test]
fn straight_line_call_order() {
    let printed = lowered("fn g(x: i64) -> i64 { x }\nfn f() -> i64 { g(1) + g(2) }");
    has(
        &printed,
        "f",
        &["_ = call g(1)", "_ = call g(2)", "_ = _ + _", "return _"],
    );
}

/// `x` is read into a temporary before the `if` that assigns it, so `g` gets the old `x`.
#[test]
fn operand_read_before_a_later_sibling_assigns() {
    let printed = lowered(
        "fn g(s: string, n: i64) -> i64 {\n    n\n}\nfn f(p: string, c: bool) -> i64 {\n    let mut x = p\n    g(x, if c {\n        x = \"new\"\n        1\n    } else { 2 })\n}",
    );
    has(
        &printed,
        "f",
        &["_ = use x#2", "branch c#1", "_ = call g(move _, _)"],
    );
}

#[test]
fn string_literal_is_a_copy() {
    let printed = lowered("fn f() { print(\"hi\") }");
    has(
        &printed,
        "f",
        &["_ = copy \"hi\"", "call print(_)", "release _"],
    );
}

/// The value first, then per level the bounds check and `make_unique`, then the store.
#[test]
fn element_write_order() {
    let printed = lowered(
        "fn v() -> string { \"v\" }\nfn f() {\n    let mut a = [[\"x\"]]\n    let i = 0\n    let j = 0\n    a[i][j] = v()\n}",
    );
    has(
        &printed,
        "f",
        &[
            "_ = call v()",
            "check a#0[i#1]",
            "make_unique a#0",
            "check a#0[i#1][j#2]",
            "make_unique a#0[i#1]",
            "_ = take a#0[i#1][j#2]",
            "a#0[i#1][j#2] = move _",
            "release _",
        ],
    );
    let printed = lowered(
        "fn v() -> i64 { 1 }\nfn f() {\n    let mut a = [[1]]\n    let i = 0\n    let j = 0\n    a[i][j] = v()\n}",
    );
    has(
        &printed,
        "f",
        &["make_unique a#0[i#1]", "a#0[i#1][j#2] = _"],
    );
    assert!(!function(&printed, "f").contains("take"), "{printed}");
}

/// A field store borrows a local base when the value leaves every local as it was (B5), and
/// otherwise reads the base into a temporary first and releases it after the store.
#[test]
fn field_store_base_borrowed_or_copied() {
    let printed = lowered(
        "class O {\n    mut n: i64\n}\nfn g(o: O) -> i64 { 1 }\nfn f(o: O) {\n    o.n = 1\n}\nfn h(o: O) {\n    o.n = g(o)\n}",
    );
    has(&printed, "f", &["o#0.0 = 1"]);
    assert!(!function(&printed, "f").contains("use"), "{printed}");
    has(
        &printed,
        "h",
        &["_ = use o#0", "_ = call g(use o#0)", "_.0 = _", "release _"],
    );
}

/// The base of an element write's field root is read into a temporary, as `place_operands`
/// evaluates it today, even when the value is a constant.
#[test]
fn field_root_of_element_write_is_not_borrowed() {
    let printed = lowered("class O {\n    mut xs: [i64]\n}\nfn f(o: O) {\n    o.xs[0] = 5\n}");
    has(
        &printed,
        "f",
        &[
            "_ = use o#0",
            "check _.0[0]",
            "make_unique _.0",
            "_.0[0] = 5",
            "release _",
        ],
    );
}

#[test]
fn nested_read_copies_the_middle_level() {
    let printed = lowered("fn f(a: [[string]], i: i64, j: i64) {\n    let x = a[i][j]\n}");
    has(
        &printed,
        "f",
        &["_ = copy a#0[i#1]", "x#3 = copy _[j#2]", "release _"],
    );
}

/// A counted local's new value is computed before its old one is released.
#[test]
fn whole_local_reassignment() {
    let printed = lowered(
        "fn f() {\n    let mut s = \"a\"\n    let t = \"b\"\n    s = t + \"x\"\n    s += \"x\"\n}",
    );
    has(
        &printed,
        "f",
        &[
            "_ = concat(use t#1, move _)",
            "release? s#0",
            "s#0 = move _",
            "_ = concat(use s#0, move _)",
            "release? s#0",
            "s#0 = move _",
        ],
    );
}

/// The spec's `Append` row, in its order.
#[test]
fn append_to_an_element() {
    let printed = lowered("fn f() {\n    let mut a = [\"x\"]\n    let i = 0\n    a[i] += \"!\"\n}");
    has(
        &printed,
        "f",
        &[
            "_ = copy a#0[i#1]",
            "_ = copy \"!\"",
            "check a#0[i#1]",
            "make_unique a#0",
            "_ = take a#0[i#1]",
            "release _",
            "_ = concat(move _, move _)",
            "a#0[i#1] = move _",
        ],
    );
}

/// The checker spills `"a"` into a local before the operand that can return, and the return
/// edge releases it.
#[test]
fn early_exit_releases_spilled_locals() {
    let printed = lowered(
        "fn f(c: bool) -> string { \"a\" + (if c { return \"r\" } else { \"b\" }) }\n\
         fn g(c: bool) -> [string] { [\"a\", if c { return [\"r\"] } else { \"b\" }] }\n\
         fn h(c: bool) -> string { \"a{if c { return \"r\" } else { \"b\" }}\" }",
    );
    let edge = "\n    release? _\n    return move _";
    has(&printed, "f", &[&format!("_ = copy \"r\"{edge}")]);
    has(&printed, "g", &[&format!("_ = array[move _]{edge}")]);
    has(&printed, "h", &[&format!("_ = copy \"r\"{edge}")]);
}

/// Lowering creates a block only when an edge reaches it.
#[test]
fn every_block_is_reachable() {
    let program = lowered_program(
        "fn f() -> i64 { panic(\"x\") }\n\
         fn g() {\n    while true {\n    }\n}\n\
         fn h(c: bool) -> i64 { if c { return 1 } else { return 2 } }\n\
         fn r() -> i64 {\n    return 1\n    print(2)\n    3\n}",
    );
    for f in &program.functions {
        assert_eq!(unreachable_blocks(f), [], "fn {}", f.name);
    }
}

/// The reachability check above sees a cycle that no edge from `bb0` enters.
#[test]
fn unreachable_blocks_finds_a_disconnected_cycle() {
    let block = |term| Block {
        stmts: Vec::new(),
        term,
    };
    let f = Function {
        name: "f".to_owned(),
        params: 0,
        ret: Type::UNIT,
        locals: Vec::new(),
        blocks: vec![
            block(Terminator::Return(None)),
            block(Terminator::Goto(BlockId(2))),
            block(Terminator::Goto(BlockId(1))),
        ],
        loops: Vec::new(),
        owners: FxHashMap::default(),
    };
    assert_eq!(unreachable_blocks(&f), [BlockId(1), BlockId(2)]);
}

/// Store order: the value runs before the write reads its root array, so a value that copies
/// `a` into `b` leaves `b`'s element as it was, and one that replaces `d` writes into the new
/// array. `tests/run/store_order.sisu` runs the same programs.
#[test]
fn store_order_value_before_the_root_array() {
    let printed = lowered(
        "fn f() {\n    let mut a = [1, 2]\n    let mut b = [0]\n    a[0] = if true {\n        b = a\n        10\n    } else {\n        20\n    }\n}\n\
         fn g() {\n    let mut d = [1, 2]\n    d[0] = if true {\n        d = [5, 6]\n        9\n    } else {\n        0\n    }\n}",
    );
    has(
        &printed,
        "f",
        &[
            "_ = use a#0",
            "b#1 = move _",
            "check a#0[0]",
            "make_unique a#0",
            "a#0[0] = _",
        ],
    );
    has(
        &printed,
        "g",
        &[
            "d#0 = move _",
            "check d#0[0]",
            "make_unique d#0",
            "d#0[0] = _",
        ],
    );
}

/// Store order: the index runs before the value, and the bounds check after it, so the value's
/// output comes before an out-of-bounds panic (`tests/run-fail/store_order_bounds.sisu`).
#[test]
fn store_order_index_then_value_then_check() {
    let printed = lowered(
        "fn noisy(n: i64) -> i64 {\n    print(n)\n    n\n}\n\
         fn f() {\n    let mut c = [0]\n    c[noisy(0)] = noisy(7)\n    c[5] = noisy(3)\n}",
    );
    has(
        &printed,
        "f",
        &[
            "_ = call noisy(0)",
            "_ = call noisy(7)",
            "check c#0[_]",
            "c#0[_] = _",
            "_ = call noisy(3)",
            "check c#0[5]",
            "c#0[5] = _",
        ],
    );
}

/// Store order: a field root's base, and a field store's base, run before the value, so a value
/// that replaces `o` writes into the old object.
#[test]
fn store_order_field_base_before_the_value() {
    let printed = lowered(
        "class Box {\n    mut xs: [i64]\n}\nclass Holder {\n    mut v: i64\n}\n\
         fn noisy(n: i64) -> i64 {\n    print(n)\n    n\n}\n\
         fn get(h: Holder, n: i64) -> Holder {\n    print(n)\n    h\n}\n\
         fn f() {\n    let mut o = Box(xs: [1])\n    o.xs[0] = if true {\n        o = Box(xs: [2])\n        3\n    } else {\n        0\n    }\n}\n\
         fn g(h: Holder) {\n    get(h, 1).v = noisy(2)\n}",
    );
    has(
        &printed,
        "f",
        &[
            "_ = use o#0",
            "o#0 = move _",
            "check _.0[0]",
            "make_unique _.0",
            "_.0[0] = _",
            "release _",
        ],
    );
    has(
        &printed,
        "g",
        &[
            "_ = call get(use h#0, 1)",
            "_ = call noisy(2)",
            "_.0 = _",
            "release _",
        ],
    );
}

/// `mentions` finds a local wherever an expression or a statement holds it: each named
/// local of `f` stands in one kind of node.
#[test]
fn mentions_every_kind_of_node() {
    let src = "class O {\n    mut n: i64\n    mut s: string\n}\n\
               enum E {\n    V(i64)\n    W\n}\n\
               fn g(x: i64) -> i64 { x }\n\
               fn f(call: i64, new: i64, math: f64, len: [i64], chars: string, bytes: string, \
               words: string, from: [char], sum: [i64], field: O, wrap: i64, convert: i64, \
               unary: i64, binary: i64, concat: string, equal: i64, is1: O, is2: O, \
               fill: i64, index: [i64], contains: [i64], compare: i64, part: string, lit: i64, \
               variant: i64, cond: bool, some: i64?, scrutinee: E, guard: i64, looped: bool, \
               assigned: i64, placed: O, at: i64, root: i64, appended: O, returned: i64, panicked: string) -> i64 {\n\
               print(g(call))\n\
               let made = O(n: new, s: \"\")\n\
               print(math.abs())\n\
               print(len.len())\n\
               print(chars.chars().len())\n\
               print(bytes.bytes().len())\n\
               print(words.words().len())\n\
               print(string(from))\n\
               print(sum.sum())\n\
               print(field.n)\n\
               let wrapped: i64? = wrap\n\
               let converted = u8(convert)\n\
               print(-unary)\n\
               print(binary + 1)\n\
               print(concat + \"a\")\n\
               print(equal == 1)\n\
               print(is1 is is2)\n\
               print([fill; 2].len())\n\
               print(index[0])\n\
               print(contains.contains(1))\n\
               print(1 < compare < 3)\n\
               print(\"{part}\")\n\
               print([lit].len())\n\
               let v = E.V(variant)\n\
               if cond { print(1) } else { print(2) }\n\
               if let n = some { print(n) }\n\
               match scrutinee {\n        V(j) if j > guard => print(j)\n        _ => print(0)\n    }\n\
               while looped { break }\n\
               let mut local = 0\n\
               local = assigned\n\
               placed.n = 1\n\
               let mut xs = [1]\n\
               xs[at] = root\n\
               appended.s += \"x\"\n\
               if cond { return returned }\n\
               panic(panicked)\n\
               }\nfn main() {}";
    let program = checked(src);
    let f = program
        .functions
        .iter()
        .find(|f| f.name == "f")
        .expect("the source should define `f`");
    let mut found = Vec::new();
    block_mentions(&f.body, &mut found);
    let found: FxHashSet<_> = found.into_iter().collect();
    for (id, local) in f.locals.iter().enumerate() {
        if let Some(name) = &local.name {
            assert!(found.contains(&id), "`{name}` not found");
        }
    }
}
