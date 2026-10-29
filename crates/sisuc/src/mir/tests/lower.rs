use rustc_hash::FxHashMap;

use super::*;
use crate::mir::lower::{Mentions, WALKED};
use crate::mir::print::EXAMINED;

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

/// An `i64` operand is pinned as a string one is: `g` gets the `n` from before the `if`.
#[test]
fn uncounted_operand_read_before_a_later_sibling_assigns() {
    let printed = lowered(
        "fn g(a: i64, b: i64) -> i64 { a }\nfn f(c: bool) -> i64 {\n    let mut n = 1\n    g(n, if c {\n        n = 2\n        1\n    } else { 2 })\n}",
    );
    has(
        &printed,
        "f",
        &["n#1 = 1", "_ = n#1", "branch c#0", "_ = call g(_, _)"],
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

/// `print` reads a compound local in place, as it reads a `string` local: `e` is used again, so
/// the first print borrows it and takes no copy.
#[test]
fn printing_a_compound_local_should_read_it_in_place() {
    let printed = built(
        "enum Expr {\n    Num(i64)\n    Add(Expr, Expr)\n}\nfn main() {\n    let e = Expr.Add(Expr.Num(1), Expr.Num(2))\n    print(e)\n    print(e)\n}",
    );
    let f = function(&printed, "main");
    assert!(f.lines().any(|l| l.trim() == "_ = borrow e#0"), "{f}");
    assert!(!f.contains("= copy e#"), "{f}");
    assert!(!f.contains("[calls]"), "{f}");
}

/// `dbg(e)` runs `e` once into a temporary, which the `dbg` statement reads in place and the
/// addition reads again: the same temporary all three times.
#[test]
fn dbg_should_read_the_temporary_that_holds_the_value() {
    let printed = lowered(
        "fn g(x: i64) -> i64 { x }\nfn f(x: i64) -> i64 {\n    let y = dbg(g(x)) + 1\n    y\n}",
    );
    let body = &printed[printed.find("fn f(").expect("`f` should be printed")..];
    let temp = body
        .lines()
        .find_map(|line| line.trim().strip_suffix(" = call g(x#0)"))
        .expect("the call of `g` should be assigned to a temporary");
    for part in [
        format!("call dbg({temp}, \"g(x)\")"),
        format!("y#1 = {temp} + 1"),
    ] {
        assert!(body.contains(&part), "`{part}` in\n{body}");
    }
    has(
        &printed,
        "f",
        &["= call g(x#0)", "call dbg(", "y#1 = _ + 1"],
    );
    assert_eq!(body.matches("call g(").count(), 1, "{body}");
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

/// The base of an element write's field root is read into a temporary, never borrowed, even
/// when the value is a constant.
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
    let program = lowered_program(UNREACHABLE_CODE);
    for f in &program.functions {
        assert_eq!(unreachable_blocks(f), [], "fn {}", f.name);
    }
}

/// The reachability check above sees a cycle that no edge from `bb0` enters.
#[test]
fn unreachable_blocks_should_find_a_disconnected_cycle() {
    let block = |term| Block {
        stmts: Vec::new(),
        term,
    };
    let f = Function {
        name: "f".to_owned(),
        params: 0,
        borrowed: Vec::new(),
        ret: Type::UNIT,
        prelude: false,
        quiet: false,
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
    let mentions = Mentions::of(&f.body, f.locals.len());
    for (id, local) in f.locals.iter().enumerate() {
        if let Some(name) = &local.name {
            assert!(mentions.mentions(id), "`{name}` not found");
        }
    }
}

/// Each kind of expression lowers to its statement: borrowed operands go to temporaries
/// released after, consumed ones are `use`d or moved, and a chain or `&&` branches.
#[test]
fn every_kind_of_node_should_lower() {
    let printed = lowered(EVERY_KIND);
    has(
        &printed,
        "f",
        &[
            "fn f(o#0: O, p#1: O, xs#2: [i64], s#3: string, cs#4: [char], c#5: bool, x#6: f64, k#7: char) -> unit {",
            "_ = copy \"\"",
            "m#8 = new O(1, move _)",
            "v#9 = variant E.V(2)",
            "_ = call g(3)",
            "w#10 = wrap _",
            "none#11 = None",
            "_ = o#0.0",
            "b#12 = convert _",
            "_ = math abs(x#6)",
            "call print(_)",
            "_ = o#0.0",
            "_ = -_",
            "_ = 1.5 * x#6",
            "_ = k#7 == 'a'",
            "_ = use s#3",
            "_ = copy \"t\"",
            "_ = _ != _\n    release _\n    release _",
            "_ = use o#0",
            "_ = use p#1",
            "_ = _ is _\n    release _\n    release _",
            "_ = xs#2[0]",
            "_ = 1 < _",
            "branch _ bb1 bb2",
            "_ = _ < 3",
            "_ = c#5",
            "branch _ bb3 bb4",
            "_ = len xs#2",
            "_ = _ > 0",
            "_ = c#5",
            "branch _ bb6 bb5",
            "_ = false",
            "_ = contains(use xs#2, 2)",
            "_ = array[4; 2]",
            "_ = call [i64].sum(move _)",
            "_ = len_words s#3",
            "ws#13 = words s#3",
            "bs#14 = bytes s#3",
            "chs#15 = chars s#3",
            "_ = string cs#4\n    call print(_)\n    release _",
            "_ = interp(fmt(x#6, \">8\"))",
            "line#16 = call read_line()",
            "call eprint(1)",
            "_ = call name()\n    release _",
            "branch c#5 bb10 bb11",
            "bb10:\n    goto bb7",
            "release? line#16",
            "return",
        ],
    );
}

/// A comparison chain branches past its later steps, `&&` and `||` skip their right side, two
/// strings compare as borrowed operands, and `return` and `panic` end their block.
#[test]
fn lowers_chains_and_exits() {
    let printed = lowered(
        "const NAN = 0.0 / 0.0\n\
         fn q(s: string, x: i64) -> bool {\n\
             print(s)\n\
             print(NAN)\n\
             let small: u8 = 5\n\
             let lt = 1 < x\n\
             let either = lt || x > 2\n\
             print(1 < 2 < x <= 9)\n\
             print(lt && s < \"u\")\n\
             panic(s)\n\
         }\n\
         fn r(c: bool) {\n\
             if c { return }\n\
             print(1)\n\
         }",
    );
    has(
        &printed,
        "q",
        &[
            "_ = use s#0\n    call print(_)\n    release _",
            "call print(nan)",
            "small#2 = 5u8",
            "lt#3 = 1 < x#1",
            "_ = lt#3",
            "branch _ bb2 bb1",
            "_ = x#1 > 2",
            "bb2:\n    either#4 = _",
            "_ = 1 < 2",
            "branch _ bb3 bb4",
            "_ = 2 < x#1",
            "branch _ bb5 bb4",
            "_ = lt#3",
            "bb5:\n    _ = x#1 <= 9\n    goto bb4",
            "_ = use s#0",
            "_ = copy \"u\"",
            "_ = _ < _\n    release _\n    release _",
            "panic use s#0",
        ],
    );
    has(
        &printed,
        "r",
        &[
            "branch c#0 bb1 bb2",
            "bb1:\n    return\n",
            "bb2:\n    call print(1)\n    return\n}",
        ],
    );
}

/// An element read reads its base in place when the index leaves every local as it was
/// (B1), and a field store its base when the value does (B5); otherwise the base is read
/// into a temporary first, and released after.
#[test]
fn base_borrowed_when_the_sibling_leaves_locals() {
    let printed = lowered(
        "class P {\n    mut n: i64\n    mut b: bool\n}\n\
         fn g(x: i64) -> i64 { x }\n\
         fn borrowed(xs: [i64], p: P, i: i64, u: u8) {\n\
             print(xs[-i])\n\
             print(xs[i64(u)])\n\
             print(xs[xs.len() - 1])\n\
             print(xs[p.n])\n\
             p.b = 1 < i\n\
         }\n\
         fn copied(xs: [i64], p: P, i: i64) {\n\
             print(xs[g(i)])\n\
             print(xs[i + g(1)])\n\
             p.b = 1 < g(i)\n\
         }",
    );
    has(
        &printed,
        "borrowed",
        &[
            "_ = -i#2\n    _ = xs#0[_]",
            "_ = convert u#3\n    _ = xs#0[_]",
            "_ = len xs#0\n    _ = _ - 1\n    _ = xs#0[_]",
            "_ = p#1.0\n    _ = xs#0[_]",
            "_ = 1 < i#2\n    p#1.1 = _",
        ],
    );
    assert!(!function(&printed, "borrowed").contains("use"));
    has(
        &printed,
        "copied",
        &[
            "_ = use xs#0\n    _ = call g(i#2)\n    _ = _[_]\n    release _",
            "_ = use xs#0\n    _ = call g(1)\n    _ = i#2 + _\n    _ = _[_]\n    release _",
            "_ = use p#1\n    _ = call g(i#2)\n    _ = 1 < _\n    _.1 = _\n    release _",
        ],
    );
}

/// An index whose read of another local is that local's last use still leaves `xs` as it was:
/// the sibling condition sees a read, not a move, so `xs` is borrowed, not copied.
#[test]
fn base_borrowed_when_the_sibling_moves_another_local() {
    let src =
        "fn moved(xs: [i64]) -> [i64] {\n    let ys = [true]\n    print(xs[ys.len()])\n    xs\n}";
    has(&lowered(src), "moved", &["_ = len ys#1\n    _ = xs#0[_]"]);
    // The ownership pass then moves `ys`, its last use, and still reads `xs` in place.
    has(
        &built(src),
        "moved",
        &["_ = move ys#1\n    _ = len _\n    release _\n    _ = xs#0[_]"],
    );
}

/// Lowering stops at an exit: the statements after it, which the checker keeps and warns
/// about, are not lowered.
#[test]
fn code_after_an_exit_is_not_lowered() {
    let printed = lowered(
        "fn f(c: bool) -> i64 {\n    if c { return 1 } else { return 2 }\n    print(3)\n    4\n}\n\
         fn h() -> i64 {\n    return 1\n    print(3)\n    4\n}\n\
         fn k(c: bool) -> i64 {\n    while c {\n        break\n        print(3)\n    }\n    panic(\"p\")\n    4\n}",
    );
    for name in ["f", "h", "k"] {
        let f = function(&printed, name);
        assert!(!f.contains("print") && !f.contains("= 4"), "{f}");
    }
    has(&printed, "h", &["_ = 1\n    return _\n}"]);
    has(&printed, "k", &["bb2:\n    goto bb4", "panic move _\n}"]);
}

/// The place a statement reads in place is marked a borrow site; a place on a temporary that
/// owns the base is not.
#[test]
fn borrow_sites_are_marked() {
    let program = lowered_program(
        "fn g(x: i64) -> i64 { x }\n\
         fn borrowed(xs: [i64], i: i64) -> i64 { xs[i] }\n\
         fn copied(xs: [i64], i: i64) -> i64 { xs[g(i)] }",
    );
    let read_place = |name: &str| {
        let f = program
            .functions
            .iter()
            .find(|f| f.name == name)
            .expect("the source should define the function");
        f.blocks[0]
            .stmts
            .iter()
            .find_map(|stmt| {
                if let Statement::Assign {
                    value: Rvalue::Use(Operand::Read(place)),
                    ..
                } = stmt
                    && !place.projections.is_empty()
                {
                    Some(place.clone())
                } else {
                    None
                }
            })
            .expect("the function should read an element")
    };
    assert!(read_place("borrowed").borrow);
    assert!(!read_place("copied").borrow);
}

/// Each loop records the block that enters it, its header, and its blocks, the exit not
/// among them; an outer loop comes before the loop inside it.
#[test]
fn loops_record_their_blocks() {
    let program = lowered_program(
        "fn f(c: bool, d: bool) {\n    while c {\n        while d { break }\n        print(1)\n    }\n}",
    );
    let f = program
        .functions
        .iter()
        .find(|f| f.name == "f")
        .expect("the source should define `f`");
    let loops: Vec<_> = f
        .loops
        .iter()
        .map(|l| {
            (
                l.pre_header.0,
                l.header.0,
                l.blocks.iter().map(|b| b.0).collect::<Vec<_>>(),
            )
        })
        .collect();
    // bb3, the `while`'s else, breaks to bb9; bb5 and bb6 break to bb7.
    assert_eq!(
        loops,
        [(0, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]), (2, 4, vec![4, 5, 6])]
    );
}

/// The pin rule asks one index of the body's mentions, so lowering walks each expression once:
/// a wide operand list and a right-deep sum cost a visit per node, not a walk of the later
/// operands per operand. Each `x` but the last is still pinned, as `x` comes again later.
#[test]
fn pins_walk_the_body_once() {
    let wide = 200;
    let deep = 60;
    let src = format!(
        "fn f(x: i64, y: i64) {{\n    print([{}0].len())\n    print({}x{})\n    print([x, y, x, y, 0].len())\n}}",
        "x, ".repeat(wide),
        "x + (".repeat(deep),
        ")".repeat(deep),
    );
    let program = checked(&src);
    WALKED.set(0);
    let printed = print(&lowered_of(&program), &program);
    // Print, Len, ArrayLit and its elements; Print, the sums and their operands; the last line.
    let nodes = (3 + wide + 1) + (1 + 2 * deep + 1) + (3 + 5);
    assert_eq!(WALKED.get(), nodes);
    let f = function(&printed, "f");
    assert_eq!(f.matches("_ = x#0\n").count(), (wide - 1) + deep + 1);
    has(
        &printed,
        "f",
        &["_ = x#0\n    _ = y#1\n    _ = array[_, _, x#0, y#1, 0]"],
    );
}

/// `break` and `continue` scope-release the locals the loop's body declared, and only those:
/// `s`, declared outside the loop, is released at the end of the function.
#[test]
fn loop_exits_release_the_body_locals() {
    let printed = lowered(
        "fn f(c: bool) {\n    let s = \"a\"\n    while c {\n        let t = \"b\"\n        if c { break } else { continue }\n    }\n}",
    );
    // The break edge, then the continue edge, then the function's exit.
    has(
        &printed,
        "f",
        &["  bb4:\n    release? t#2\n    goto bb6\n  \
           bb5:\n    release? t#2\n    goto bb1\n  \
           bb6:\n    release? s#1\n    return"],
    );
}

/// A value `loop`'s `break`s each write the one local that its exit block returns.
#[test]
fn value_loop_writes_one_destination() {
    let printed = lowered(
        "fn f(c: bool, s: string) -> string {\n    loop {\n        if c { break s }\n        break \"x\"\n    }\n}",
    );
    let start = printed.find("fn f(").expect("the source should define `f`");
    let f = &printed[start..];
    let f = &f[..f.find("\n}").map_or(f.len(), |end| end + 2)];
    let returns: Vec<_> = f.match_indices("return move ").map(|(at, _)| at).collect();
    assert_eq!(returns.len(), 1, "{f}");
    let returned = f[returns[0]..]
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("return move "))
        .expect("the return should move a local");
    let writes: Vec<_> = f
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix(&format!("{returned} = ")))
        .collect();
    assert_eq!(writes, ["use s#1", "copy \"x\""], "{f}");
    // Both `break`s go to the block that returns.
    let exit = f[..returns[0]]
        .rfind("  bb")
        .map(|at| &f[at + 2..])
        .and_then(|rest| rest.split(':').next())
        .expect("the return should sit in a block");
    assert_eq!(f.matches(&format!("goto {exit}\n")).count(), 2, "{f}");
}

/// `break outer` and `continue outer` from the inner loop each scope-release the inner body's
/// `t` and the outer body's `s`, once, then jump out of the outer loop or to its header.
#[test]
fn labeled_jumps_release_each_scope_once() {
    let program = lowered_program(
        "fn f(c: bool) {\n    outer: while c {\n        let s = \"a\"\n        while c {\n            let t = \"b\"\n            if c { break outer } else { continue outer }\n        }\n    }\n}",
    );
    let f = program
        .functions
        .iter()
        .find(|f| f.name == "f")
        .expect("the source should define `f`");
    let named = |name: &str| {
        f.locals
            .iter()
            .position(|local| local.name.as_deref() == Some(name))
            .map_or_else(|| panic!("no local {name}"), LocalId)
    };
    let (s, t) = (named("s"), named("t"));
    // The blocks that release `t` and then `s`, and nothing else, and where each jumps.
    let edges: Vec<BlockId> = f
        .blocks
        .iter()
        .filter(|block| {
            matches!(block.stmts.as_slice(),
                [Statement::ScopeRelease(first), Statement::ScopeRelease(second)]
                    if *first == t && *second == s)
        })
        .map(|block| match block.term {
            Terminator::Goto(to) => to,
            ref term => panic!("a jump should end in a goto, not {term:?}"),
        })
        .collect();
    let outer = &f.loops[0];
    let [exit, header] = edges[..] else {
        panic!("two edges should release both scopes: {edges:?}");
    };
    assert!(
        !outer.blocks.contains(&exit),
        "{exit:?} in {:?}",
        outer.blocks
    );
    assert_eq!(header, outer.header);
    // No other block releases either local but the end of its own body.
    let releases = |local| {
        f.blocks
            .iter()
            .flat_map(|block| &block.stmts)
            .filter(|stmt| matches!(stmt, Statement::ScopeRelease(l) if *l == local))
            .count()
    };
    assert_eq!((releases(s), releases(t)), (3, 2));
}

/// The outer `break`'s operand is an inner `loop` whose `break rows 1` leaves for the outer
/// exit, and whose `break 2` completes the operand: both write the outer loop's destination,
/// and both paths reach the outer exit, the first directly and the second through the inner one.
#[test]
fn value_operand_jumps_to_another_loop() {
    let printed = lowered(
        "fn f(c: bool) -> i64 {\n    rows: loop {\n        break loop {\n            if c { break rows 1 }\n            break 2\n        }\n    }\n}",
    );
    let start = printed.find("fn f(").expect("the source should define `f`");
    let f = &printed[start..];
    let f = &f[..f.find("\n}").map_or(f.len(), |end| end + 2)];
    let returns: Vec<_> = f.match_indices("return ").map(|(at, _)| at).collect();
    assert_eq!(returns.len(), 1, "{f}");
    let returned = f[returns[0]..]
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("return "))
        .expect("the return should give a local");
    let writes: Vec<_> = f
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix(&format!("{returned} = ")))
        .collect();
    assert_eq!(writes, ["1", "2"], "{f}");
    let exit = f[..returns[0]]
        .rfind("  bb")
        .map(|at| &f[at + 2..])
        .and_then(|rest| rest.split(':').next())
        .expect("the return should sit in a block");
    assert_eq!(f.matches(&format!("goto {exit}\n")).count(), 2, "{f}");
}

/// A `break` whose value never completes makes no exit: these loops leave only by a panic, an
/// endless inner loop or a `return`, so the function returns at most where `return` stands, and
/// every block is reachable.
#[test]
fn break_without_a_value_edge_makes_no_exit() {
    for (name, body, returns) in [
        ("break_panic", "loop { break panic(\"x\") }", 0),
        ("break_endless_loop", "loop { break loop {} }", 0),
        ("break_after_return", "loop { return 1; break 2 }", 1),
    ] {
        let program = lowered_program(&format!("fn f() -> i64 {{\n    {body}\n}}"));
        let f = program
            .functions
            .iter()
            .find(|f| f.name == "f")
            .expect("the source should define `f`");
        let found = f
            .blocks
            .iter()
            .filter(|b| matches!(b.term, Terminator::Return(_)))
            .count();
        assert_eq!(found, returns, "{name}");
        assert_eq!(unreachable_blocks(f), [], "{name}");
    }
}

/// An interpolation consumes its parts, so a local that is used again is copied once, whether it
/// is a `string` (`"{s}"`) or a compound value (`"{e}"`): one retain and one release.
#[test]
fn interpolating_a_compound_local_should_copy_it_once() {
    let decls = "enum Expr {\n    Num(i64)\n    Add(Expr, Expr)\n}\n";
    for (ty, name) in [("string", "s"), ("Expr", "e")] {
        let src = format!(
            "{decls}fn f({name}: {ty}) -> string {{\n    let t = \"{{{name}}}\"\n    print({name})\n    t\n}}\nfn main() {{}}"
        );
        let f = function(&built(&src), "f");
        assert_eq!(
            f.matches(&format!("= copy {name}#0")).count(),
            1,
            "{ty}: {f}"
        );
        assert!(f.contains("interp("), "{ty}: {f}");
        assert!(!f.contains("[calls]"), "{ty}: {f}");
    }
}

/// Each part of an interpolation prints with its own format, in part order.
#[test]
fn interpolation_prints_each_format_with_its_part() {
    let printed =
        lowered("fn f(a: i64, b: i64, c: i64, d: i64) {\n    print(\"{a:>3}{b}{c:<2}{d:>4}\")\n}");
    has(
        &printed,
        "f",
        &["interp(fmt(a#0, \">3\"), b#1, fmt(c#2, \"<2\"), fmt(d#3, \">4\"))"],
    );
}

/// The printer meets the formats in one pass: a wide interpolation examines one format entry
/// per part, not a search of the formats from the start for each part.
#[test]
fn interpolation_examines_each_format_once() {
    let parts = 200;
    let src = format!(
        "fn f(a: i64) {{\n    print(\"{}\")\n}}",
        "{a:>3}".repeat(parts)
    );
    let program = checked(&src);
    let mir = lowered_of(&program);
    EXAMINED.set(0);
    let printed = print(&mir, &program);
    assert_eq!(EXAMINED.get(), parts);
    assert_eq!(printed.matches(", \">3\")").count(), parts);
}

/// The present value of an optional prints as a pattern would bind it.
#[test]
fn some_projection_prints_as_a_pattern() {
    let program = checked("");
    let mut b = Builder::new("f");
    let p = b.param("p", Type::optional(Type::STRING));
    let x = b.local("x", Type::STRING);
    b.block(
        vec![
            assign(
                x,
                Rvalue::Borrow(Place {
                    projections: vec![Projection::Some],
                    ..whole(p)
                }),
            ),
            Statement::End(x),
        ],
        Terminator::Return(None),
    );
    let mir = Program {
        functions: vec![b.finish()],
    };
    has(&print(&mir, &program), "f", &["x#1 = borrow (p#0 as some)"]);
}

/// A variant pattern on a `Shape?` tests that a value is present, then switches on the tag of
/// the value inside, and binds through `Some` in the middle of the path.
#[test]
fn a_variant_pattern_on_an_optional_should_test_presence_then_the_tag() {
    let printed = lowered(
        "enum Shape {\n    Circle(f64)\n    Square(f64)\n}\nfn f(s: Shape?) -> f64 {\n    match s {\n        Circle(r) => r\n        Square(w) => w\n        None => 0.0\n    }\n}",
    );
    has(
        &printed,
        "f",
        &[
            "test_present _ bb",
            "switch tag (_ as some) [0: bb",
            "((_ as some) as 0).0",
        ],
    );
}

/// `V(Circle(r))` on an owned `Shape?` payload takes the scrutinee apart through `Some`, an
/// inline step: `r` moves out, with no token, no copy and no release of the scrutinee.
#[test]
fn a_take_apart_should_step_through_an_inline_some() {
    let printed = built(
        "enum Shape {\n    Circle(string)\n    Square(i64)\n}\nenum H {\n    V(Shape?)\n    W(string)\n}\nfn f(h: H) -> string {\n    match h {\n        V(Circle(r)) => r\n        _ => \"\"\n    }\n}",
    );
    has(
        &printed,
        "f",
        &[
            "test_present (_ as 0).0 bb",
            "take_apart _: r#",
            "= (((_ as 0).0 as some) as 0).0",
        ],
    );
    let f = function(&printed, "f");
    assert!(!f.contains("token"), "a token in\n{f}");
}

/// A pattern under a boxed `E?` field tests the field's presence, then the tag of the enum in
/// its box.
#[test]
fn a_pattern_under_a_boxed_optional_field_should_test_presence_then_the_tag() {
    let printed = lowered(
        "enum E {\n    End\n    Next(E?)\n}\nfn f(e: E) -> i64 {\n    match e {\n        Next(Some(Next(_))) => 2\n        _ => 0\n    }\n}",
    );
    has(
        &printed,
        "f",
        &[
            "switch tag e#0",
            "test_present (e#0 as 1).0 bb",
            "switch tag ((e#0 as 1).0 as some)",
        ],
    );
}

/// Under a boxed `T?` field the box stays in the rest: each counted binding under it is
/// copied once, though two alternatives bind it, and an uncounted one is not copied.
#[test]
fn a_binding_under_a_boxed_optional_field_should_be_copied_once() {
    let printed = built(
        "enum T {\n    End\n    Next(T?, string, i64)\n}\nfn f(t: T) -> string {\n    match t {\n        Next(Some(Next(None, s, n) | Next(Some(_), s, n)), _, _) => \"{s}{n}\"\n        _ => \"\"\n    }\n}",
    );
    let f = function(&printed, "f");
    assert_eq!(f.matches(" = copy s#").count(), 1, "{f}");
    assert!(!f.contains(" = copy n#"), "a copy of `n` in\n{f}");
    assert!(!f.contains("token"), "a token in\n{f}");
    has(&printed, "f", &["take_apart _: rest _"]);
}
