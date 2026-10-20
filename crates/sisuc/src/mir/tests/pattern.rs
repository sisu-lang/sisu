use super::*;
use crate::mir::lower::PROBED;

/// The function `name` of `program`.
fn func<'p>(program: &'p Program, name: &str) -> &'p Function {
    program
        .functions
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no function {name}"))
}

/// The local of `f` named `name`.
fn named(f: &Function, name: &str) -> LocalId {
    let at = f
        .locals
        .iter()
        .position(|local| local.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no local {name} in fn {}", f.name));
    LocalId(at)
}

/// The locals of `f` named `name`, in order.
fn all_named(f: &Function, name: &str) -> Vec<LocalId> {
    (0..f.locals.len())
        .filter(|&i| f.locals[i].name.as_deref() == Some(name))
        .map(LocalId)
        .collect()
}

/// The blocks of `f` whose terminator `pick` accepts.
fn blocks_where(f: &Function, pick: impl Fn(&Terminator) -> bool) -> Vec<BlockId> {
    (0..f.blocks.len())
        .filter(|&i| pick(&f.blocks[i].term))
        .map(BlockId)
        .collect()
}

/// The `else` block of the one `branch` on `cond`.
fn else_of_branch_on(f: &Function, cond: LocalId) -> BlockId {
    let found: Vec<_> = f
        .blocks
        .iter()
        .filter_map(|block| {
            if let Terminator::Branch {
                cond: Operand::Read(place),
                else_,
                ..
            } = &block.term
                && place.local == cond
            {
                Some(*else_)
            } else {
                None
            }
        })
        .collect();
    let [else_] = found[..] else {
        panic!("one branch on {cond:?} in fn {}, not {found:?}", f.name)
    };
    else_
}

/// The `switch` on `tried` that ends `block`, if one does: its arms, each with the value of
/// `tried` it takes, and its otherwise.
fn switch_on(
    f: &Function,
    block: BlockId,
    tried: LocalId,
) -> Option<(Vec<(i64, BlockId)>, BlockId)> {
    if let Terminator::SwitchValue {
        operand: Operand::Read(place),
        arms,
        otherwise: Some(otherwise),
    } = &f.blocks[block.0].term
        && place.local == tried
    {
        let arms = arms
            .iter()
            .map(|(key, to)| {
                let Const::Int(key, _) = key else {
                    panic!("a switch on `tried` should take integers, not {key:?}")
                };
                (*key, *to)
            })
            .collect();
        Some((arms, *otherwise))
    } else {
        None
    }
}

/// The dispatch block of the choice point `tried`: where its `tried = 0` jumps.
fn dispatch_of(f: &Function, tried: LocalId) -> BlockId {
    let starts: Vec<_> = f
        .blocks
        .iter()
        .filter_map(|block| {
            if let (
                Some(Statement::Assign {
                    dest: Some(dest),
                    value: Rvalue::Use(Operand::Const(Const::Int(0, _))),
                }),
                Terminator::Goto(to),
            ) = (block.stmts.last(), &block.term)
                && *dest == tried
            {
                Some(*to)
            } else {
                None
            }
        })
        .collect();
    let [dispatch] = starts[..] else {
        panic!("one `tried = 0` for {tried:?}, not {starts:?}")
    };
    dispatch
}

/// `if let` moves the optional into a temporary, and the `Some` edge moves the payload out of
/// it: no copy and no release of that temporary on either edge. An uncounted payload is read,
/// and its binding has nothing to release.
#[test]
fn ifsome_moves_the_payload() {
    let printed = lowered(
        "fn unwrap(p: string?) -> string {\n    let r = p\n    if let q = r { q } else { return \"none\" }\n}\n\
         fn count(m: i64?) -> i64 { if let q = m { q } else { 0 } }",
    );
    has(
        &printed,
        "count",
        &["if_present _ bb1 bb2", "q#1 = unwrap _"],
    );
    assert!(!function(&printed, "count").contains("release"));
    has(
        &printed,
        "unwrap",
        &[
            "_ = use r#1",
            "if_present _ bb1 bb2",
            "bb1:",
            "q#2 = unwrap move _",
        ],
    );
    let f = function(&printed, "unwrap");
    for absent in ["copy _", "release _", "release? _"] {
        assert!(!f.contains(absent), "`{absent}` in\n{f}");
    }
}

/// Three arms test in arm order, not tag order: each failure goes to the next arm's test, and
/// the last arm's failure is `unreachable`.
#[test]
fn arms_test_in_order() {
    let src = "enum E {\n    A(i64)\n    B(i64)\n    C(i64)\n}\n\
               fn f(e: E) -> i64 {\n    match e {\n        C(x) => x\n        A(y) => y\n        B(z) => z\n    }\n}";
    has(
        &lowered(src),
        "f",
        &[
            "switch tag _ [2: bb",
            "] otherwise bb",
            "switch tag _ [0: bb",
            "switch tag _ [1: bb",
        ],
    );
    let program = lowered_program(src);
    let f = func(&program, "f");
    let mut block = BlockId(0);
    for tag in [2, 0, 1] {
        let Terminator::SwitchTag {
            arms, otherwise, ..
        } = &f.blocks[block.0].term
        else {
            panic!("bb{} should test a tag", block.0)
        };
        assert_eq!(arms.iter().map(|(t, _)| *t).collect::<Vec<_>>(), [tag]);
        block = otherwise.expect("a tag test should have a failure edge");
    }
    assert!(matches!(f.blocks[block.0].term, Terminator::Unreachable));
}

/// A range tests both bounds and joins them with `and`; a `bool` pattern is a `branch`.
#[test]
fn range_and_bool_patterns() {
    let printed = lowered(
        "fn r(n: i64) -> i64 {\n    match n {\n        1..=5 => 1\n        _ => 0\n    }\n}\n\
         fn b(c: bool) -> i64 {\n    match c {\n        true => 1\n        false => 0\n    }\n}",
    );
    has(
        &printed,
        "r",
        &["_ = _ >= 1", "_ = _ <= 5", "_ = _ && _", "branch _ bb"],
    );
    has(&printed, "b", &["branch _ bb"]);
    assert!(!function(&printed, "b").contains("switch"));
}

/// A literal tests with a `switch` on the value, an `i64`, a `u8` or a string; a `char` range
/// compares with code points.
#[test]
fn literals_switch_on_the_value() {
    let printed = lowered(
        "fn f(n: i64, b: u8, ss: [string], c: char) -> i64 {\n\
             let x = match n {\n        3 => 1\n        _ => 0\n    }\n\
             let y = match b {\n        7 => 1\n        _ => 0\n    }\n\
             let z = match ss[0] {\n        \"hi\" => 1\n        _ => 0\n    }\n\
             let w = match c {\n        'a'..='z' => 1\n        _ => 0\n    }\n\
             x + y + z + w\n}",
    );
    has(
        &printed,
        "f",
        &[
            "switch _ [3: bb",
            "switch _ [7u8: bb",
            "switch _ [\"hi\": bb",
            "_ = _ >= 'a'",
            "_ = _ <= 'z'",
        ],
    );
}

/// A binding borrows its part of the scrutinee while the test runs. The commit copies it and
/// then releases the scrutinee at once, before the body; no exit of the arm releases the
/// scrutinee again.
#[test]
fn bindings_borrow_then_commit_copies() {
    let src = "enum Box {\n    Full(string)\n    Empty\n}\n\
         fn f(bs: [Box], c: bool) -> string {\n    match bs[0] {\n        Full(x) => {\n            if c {\n                return x\n            }\n            x\n        }\n        Empty => \"\"\n    }\n}";
    let printed = lowered(src);
    has(
        &printed,
        "f",
        &[
            "x#2 = borrow (_ as 0).0",
            "x#2 = copy x#2\n    release? _\n    branch c#1",
        ],
    );
    // One scope release of the scrutinee per committed arm, and no other.
    assert_eq!(function(&printed, "f").matches("release? _").count(), 2);
    let program = lowered_program(src);
    let f = func(&program, "f");
    let Terminator::SwitchTag { place, .. } = &f.blocks[0].term else {
        panic!("bb0 should test the tag")
    };
    assert_eq!(
        f.owners[&named(f, "x")],
        place.local,
        "x borrows from the scrutinee"
    );
}

/// Without an `|`, a guard's false edge goes where the tag test's failure does: the next arm's
/// test. With one, it goes to the retry block, a `switch` on `tried`, not to the dispatch.
#[test]
fn guard_false_goes_to_next_arm_or_retry() {
    let program = lowered_program(
        "enum E {\n    S(string)\n    T(string)\n    U\n}\n\
         fn g(e: E, c: bool) -> i64 {\n    match e {\n        S(x) if c => 1\n        _ => 0\n    }\n}\n\
         fn h(e: E, c: bool) -> i64 {\n    match e {\n        S(x) | T(x) if c => 1\n        _ => 0\n    }\n}",
    );
    let g = func(&program, "g");
    let Terminator::SwitchTag {
        otherwise: Some(next_arm),
        ..
    } = g.blocks[0].term
    else {
        panic!("g's bb0 should test the tag")
    };
    assert_eq!(else_of_branch_on(g, named(g, "c")), next_arm);
    let h = func(&program, "h");
    let tried = named(h, "tried");
    let retry = else_of_branch_on(h, named(h, "c"));
    assert!(switch_on(h, retry, tried).is_some(), "bb{}", retry.0);
    assert_ne!(retry, dispatch_of(h, tried));
}

/// The failure edge from a guard to the next arm ends each binding's borrow. A `break` out of a
/// guard ends them too, before its releases, and releases no binding.
#[test]
fn failure_edge_ends_bindings() {
    let program = lowered_program(
        "enum P {\n    Two(string, string)\n    Zero\n}\n\
         fn f(p: P, c: bool) -> i64 {\n    match p {\n        Two(x, y) if c => 1\n        _ => 0\n    }\n}\n\
         fn g(ps: [P], c: bool) -> i64 {\n    for p in ps {\n        match p {\n            Two(x, y) if (if c { break } else { true }) => print(x)\n            _ => print(0)\n        }\n    }\n    0\n}",
    );
    let fails = func(&program, "f");
    let bindings = [named(fails, "x"), named(fails, "y")];
    let next_arm = &fails.blocks[else_of_branch_on(fails, named(fails, "c")).0].stmts;
    assert!(
        matches!(next_arm[..], [Statement::End(first), Statement::End(second), ..] if [first, second] == bindings),
        "{next_arm:?}"
    );
    let breaks = func(&program, "g");
    let bindings = [named(breaks, "x"), named(breaks, "y")];
    let inside = &breaks.loops[0];
    let exits: Vec<_> = blocks_where(breaks, |term| {
        matches!(term, Terminator::Goto(to) if !inside.blocks.contains(to) && *to != inside.header)
    })
    .into_iter()
    .filter(|block| {
        breaks.blocks[block.0]
            .stmts
            .iter()
            .any(|stmt| matches!(stmt, Statement::End(_)))
    })
    .collect();
    let [exit] = exits[..] else {
        panic!("one `break` from the guard, not {exits:?}")
    };
    let stmts = &breaks.blocks[exit.0].stmts;
    assert!(
        matches!(stmts[..], [Statement::End(first), Statement::End(second), Statement::ScopeRelease(_), ..] if [first, second] == bindings),
        "{stmts:?}"
    );
    assert!(
        !stmts
            .iter()
            .any(|stmt| matches!(stmt, Statement::ScopeRelease(local) if bindings.contains(local))),
        "{stmts:?}"
    );
}

/// The payload field that the alternative starting at `start` binds: `start` tests the tag of a
/// `T` and its match edge borrows one field of it.
fn borrowed_field(f: &Function, start: BlockId) -> usize {
    let Terminator::SwitchTag { arms, .. } = &f.blocks[start.0].term else {
        panic!("bb{} should test a `T`'s tag", start.0)
    };
    let [(_, matched)] = arms[..] else {
        panic!("bb{} should test one tag, not {arms:?}", start.0)
    };
    let Some(Statement::Assign {
        value: Rvalue::Borrow(place),
        ..
    }) = f.blocks[matched.0].stmts.first()
    else {
        panic!("bb{} should start with a binding's borrow", matched.0)
    };
    let Some(Projection::Payload { field, .. }) = place.projections.last() else {
        panic!("{place:?} should end in a payload field")
    };
    *field
}

/// Nested `|` tries alternatives in left-to-right expansion order: each dispatch tries its
/// alternatives in order, the guard's failure resumes the last choice point, and once that one
/// runs out it resumes the first. Each choice point has one `tried` and one dispatch block, and
/// the guard is one block.
#[test]
fn order_of_nested_alternatives() {
    let src = "enum Pair {\n    T(string, string)\n}\n\
               enum Ds {\n    D(Pair, Pair)\n    N\n}\n\
               fn see(a: string, b: string) -> bool { a == b }\n\
               fn f(d: Ds) -> i64 {\n    match d {\n        D(T(x, _) | T(_, x), T(y, _) | T(_, y)) if see(x, y) => 1\n        _ => 0\n    }\n}";
    assert_eq!(function(&lowered(src), "f").matches("call see(").count(), 1);
    let program = lowered_program(src);
    let f = func(&program, "f");
    let tried = all_named(f, "tried");
    let [first, last] = tried[..] else {
        panic!("one `tried` per choice point, not {tried:?}")
    };
    let guard = blocks_where(f, |term| matches!(term, Terminator::Branch { .. }));
    let [guard] = guard[..] else {
        panic!("one guard block, not {guard:?}")
    };
    let Terminator::Branch { else_: retry, .. } = f.blocks[guard.0].term else {
        unreachable!("`blocks_where` picked a branch")
    };
    let (_, exhausted) =
        switch_on(f, retry, last).expect("the guard's failure resumes the last choice point");
    let (_, last_exhausted) =
        switch_on(f, dispatch_of(f, last), last).expect("a dispatch switches on its `tried`");
    assert_eq!(exhausted, last_exhausted);
    assert!(
        switch_on(f, last_exhausted, first).is_some(),
        "the last choice point, run out, resumes the first"
    );
    assert_ne!(last_exhausted, dispatch_of(f, first));
    for tried in [first, last] {
        let (starts, _) = switch_on(f, dispatch_of(f, tried), tried).expect("a dispatch switches");
        let mut fields: Vec<_> = starts
            .iter()
            .map(|&(key, start)| (key, borrowed_field(f, start)))
            .collect();
        fields.sort_unstable();
        // `tried = 0` starts at `T(x, _)`, which binds field 0, and `tried = 1` at `T(_, x)`.
        assert_eq!(fields, [(0, 0), (1, 1)]);
    }
}

/// Whether a read in a guard reads a binding is one probe, not a scan of the open guards'
/// bindings: a guard that reads each of `wide` bindings probes once per read.
#[test]
fn wide_guard_probes_once_per_read() {
    let wide = 64;
    let names: Vec<_> = (0..wide).map(|i| format!("a{i}")).collect();
    let reads: Vec<_> = names.iter().map(|name| format!("keep({name})")).collect();
    let src = format!(
        "enum W {{\n    V({})\n    N\n}}\nfn keep(s: string) -> bool {{ true }}\n\
         fn f() -> i64 {{\n    let w = W.N\n    match w {{\n        V({}) if {} => 1\n        _ => 0\n    }}\n}}",
        vec!["string"; wide].join(", "),
        names.join(", "),
        reads.join(" && "),
    );
    let program = checked(&src);
    PROBED.set(0);
    let mir = lowered_of(&program);
    // One probe for `use w#0`, and one per binding the guard reads.
    assert_eq!(PROBED.get(), 1 + wide);
    let f = func(&mir, "f");
    let marked = f
        .blocks
        .iter()
        .flat_map(|block| &block.stmts)
        .filter(|stmt| {
            matches!(stmt, Statement::Assign { value: Rvalue::Call { args, .. }, .. }
                if matches!(args[..], [Operand::Guarded(_)]))
        })
        .count();
    assert_eq!(marked, wide, "each guard read is marked");
}

/// `binary_trees`' `Node`.
const NODE: &str = "class Node {\n    left: Node?\n    right: Node?\n\n    fn check(self) -> i64 {\n        1 + (self.left?.check() ?? 0) + (self.right?.check() ?? 0)\n    }\n}";

/// `enum_match`'s `Expr` and `eval`.
const EVAL: &str = "enum Expr {\n    Num(i64)\n    Add(Expr, Expr)\n    Mul(Expr, Expr)\n}\n\
    fn eval(e: Expr) -> i64 {\n    match e {\n        Num(n) => n\n        Add(a, b) => eval(a) + eval(b)\n        Mul(a, b) => (eval(a) * eval(b)) % 1_000_003\n    }\n}";

/// The mir of `src` as `build` gives it, and its function `name`.
fn built_function(src: &str, name: &str) -> Function {
    build(&checked(src))
        .functions
        .into_iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no function {name}"))
}

/// How many `end`s of `local` `block` holds.
fn ends(f: &Function, block: BlockId, local: LocalId) -> usize {
    f.blocks[block.0]
        .stmts
        .iter()
        .filter(|stmt| matches!(stmt, Statement::End(ended) if *ended == local))
        .count()
}

/// The blocks of `f` that leave an arm inside its one loop by `return`, `break` or `continue`
/// and end one of `locals` on the way.
fn arm_exits(f: &Function, locals: &[LocalId]) -> Vec<BlockId> {
    let inside = &f.loops[0];
    blocks_where(f, |term| match term {
        Terminator::Return(_) => true,
        Terminator::Goto(to) => *to == inside.header || !inside.blocks.contains(to),
        _ => false,
    })
    .into_iter()
    .filter(|&block| inside.blocks.contains(&block))
    .filter(|&block| locals.iter().any(|&local| ends(f, block, local) > 0))
    .collect()
}

/// `self.left?.check()` borrows the field and the payload in place: nothing is counted, and each
/// `?.` ends the binding and the optional's borrow.
#[test]
fn method_on_an_optional_field_counts_nothing() {
    let printed = built(NODE);
    has(
        &printed,
        "Node.check",
        &[
            "fn Node.check(self#0: borrow Node)",
            "_ = borrow self#0.0",
            "= borrow (_ as some)",
        ],
    );
    let f = function(&printed, "Node.check");
    for absent in ["copy", "release"] {
        assert!(!f.contains(absent), "`{absent}` in\n{f}");
    }
    assert_eq!(f.matches("end _").count(), 4, "{f}");
}

/// `eval`'s `match e` tests the parameter in place, and its arms pass their bindings in place.
#[test]
fn match_on_a_borrowed_parameter_counts_nothing() {
    let printed = built(EVAL);
    has(
        &printed,
        "eval",
        &[
            "fn eval(e#0: borrow Expr)",
            "switch tag e#0",
            "a#2 = borrow (e#0 as 1).0",
            "call eval(a#2)",
        ],
    );
    let f = function(&printed, "eval");
    for absent in ["copy", "release"] {
        assert!(!f.contains(absent), "`{absent}` in\n{f}");
    }
}

/// The spec's `pick`: returning the binding consumes the optional it borrows from.
#[test]
fn binding_returned_from_a_borrowed_if_let_makes_the_parameter_owned() {
    let printed = built("fn pick(p: [i64]?) -> [i64] { if let q = p { q } else { [] } }");
    has(&printed, "pick", &["fn pick(p#0: [i64]?)", "unwrap move"]);
}

/// Returning a binding makes `e` owned, and `first` is lowered again in the owned form: the
/// commit copies the binding.
#[test]
fn binding_returned_from_a_borrowed_match_makes_the_parameter_owned() {
    let printed = built(&format!(
        "{EVAL}\nfn first(e: Expr) -> Expr {{\n    match e {{\n        Add(a, b) => a\n        Mul(a, b) => b\n        Num(n) => Expr.Num(n)\n    }}\n}}"
    ));
    has(
        &printed,
        "first",
        &["fn first(e#0: Expr)", "a#1 = copy a#1"],
    );
}

/// Review Focus 3: a scrutinee read through a `mut` field is copied, so a call in the arm that
/// replaces the field cannot free what the arm reads.
#[test]
fn match_through_a_mut_field_copies() {
    let printed = built(&format!(
        "{EVAL}\nclass O {{\n    mut e: Expr\n}}\n\
         fn f(o: O) -> i64 {{\n    match o.e {{\n        Num(n) => n\n        _ => 0\n    }}\n}}"
    ));
    has(&printed, "f", &["fn f(o#0: borrow O)", "copy o#0.0"]);
}

/// Sol's `pick`: the borrowed form returns `s`, which consumes `w`; the owned form, lowered
/// again, copies the field, and `w` stays owned.
#[test]
fn field_binding_returned_stays_owned() {
    let printed = built(
        "class W {\n    s: string?\n}\nfn pick(w: W) -> string { if let s = w.s { s } else { \"\" } }",
    );
    has(
        &printed,
        "pick",
        &["fn pick(w#0: W)", "_ = move w#0", "copy _.0", "unwrap move"],
    );
}

/// An `if let` that only reads its binding leaves the parameter borrowed: nothing copies or
/// releases the parameter, the optional's borrow or the binding, and `print` reads `s` in place.
/// `??` assigns its result on two paths, a join, which consumes the parameter.
#[test]
fn optional_read_in_both_arms_stays_borrowed() {
    let printed = built(
        "fn show(p: string?) {\n    if let s = p {\n        print(s)\n    } else {\n        print(\"none\")\n    }\n}\n\
         fn show_default(p: string?) {\n    print(p ?? \"none\")\n}",
    );
    has(
        &printed,
        "show",
        &["fn show(p#0: borrow string?)", "call print(s#1)"],
    );
    let show = function(&printed, "show");
    for line in show
        .lines()
        .filter(|line| line.contains("copy") || line.contains("release"))
    {
        assert!(
            !line.contains("p#0") && !line.contains("s#1"),
            "`{line}` in\n{show}"
        );
    }
    // The printer spells every temporary `_`: find the optional's borrow in the mir itself.
    let f = built_function(
        "fn show(p: string?) {\n    if let s = p {\n        print(s)\n    } else {\n        print(\"none\")\n    }\n}",
        "show",
    );
    let stmts = || f.blocks.iter().flat_map(|block| &block.stmts);
    let optional = stmts()
        .find_map(|stmt| match stmt {
            Statement::Assign {
                dest: Some(dest),
                value: Rvalue::Borrow(place),
            } if place.local == LocalId(0) => Some(*dest),
            _ => None,
        })
        .expect("`p` should be borrowed into a temporary");
    // Only borrows from it and its `end` name the optional: no copy, move or release.
    let mention = format!("{optional:?}");
    for stmt in stmts().filter(|stmt| format!("{stmt:?}").contains(&mention)) {
        assert!(
            matches!(
                stmt,
                Statement::Assign {
                    value: Rvalue::Borrow(_),
                    ..
                } | Statement::End(_)
            ),
            "{stmt:?} in {f:?}"
        );
    }
    has(&printed, "show_default", &["fn show_default(p#0: string?)"]);
}

/// `let k = e` moves `e` while `a` borrows from it, under the assumed flag. `e` comes out owned,
/// and `split` is lowered again in the owned form, which the certifier accepts.
#[test]
fn re_lowering_keeps_a_consumed_root_sound() {
    let printed = built(&format!(
        "{EVAL}\nfn split(e: Expr) -> i64 {{\n    match e {{\n        Add(a, b) => {{\n            let k = e\n            eval(a) + eval(k)\n        }}\n        Mul(a, b) => 0\n        Num(n) => n\n    }}\n}}"
    ));
    has(&printed, "split", &["fn split(e#0: Expr)"]);
}

/// Review Focus 3: an element is no borrowed place, so `if let` copies it.
#[test]
fn if_let_on_an_element_copies() {
    let printed = built(&format!(
        "{NODE}\nfn f(ns: [Node?]) -> i64 {{ if let n = ns[0] {{ n.check() }} else {{ 0 }} }}"
    ));
    has(&printed, "f", &["copy ns#0[0]", "n#1 = unwrap move"]);
}

/// Review Focus 2: each `return`, `break` and `continue` out of a borrowed arm ends that arm's
/// bindings once, and the certifier accepts `f`.
#[test]
fn borrowed_arm_exits_end_its_bindings() {
    let src = format!(
        "{EVAL}\nfn f(e: Expr, c: bool) -> i64 {{\n    let mut k = 0\n    while k < 3 {{\n        k += 1\n        match e {{\n            Add(a, b) => {{\n                if c {{\n                    return eval(a)\n                }}\n                break\n            }}\n            Mul(a, b) => {{\n                continue\n            }}\n            Num(n) => {{\n                k += n\n            }}\n        }}\n    }}\n    k\n}}"
    );
    has(&built(&src), "f", &["fn f(e#0: borrow Expr"]);
    let f = built_function(&src, "f");
    let (a, b) = (all_named(&f, "a"), all_named(&f, "b"));
    let arms = [[a[0], b[0]], [a[1], b[1]]];
    let exits = arm_exits(&f, &[a[0], b[0], a[1], b[1]]);
    assert_eq!(
        exits.len(),
        3,
        "a return, a break and a continue: {exits:?}"
    );
    for block in exits {
        let arm = arms
            .iter()
            .find(|arm| ends(&f, block, arm[0]) > 0)
            .unwrap_or_else(|| panic!("bb{} should end an arm's first binding", block.0));
        for &binding in arm {
            assert_eq!(ends(&f, block, binding), 1, "bb{}: {f:?}", block.0);
        }
    }
}

/// Review Focus 2: each `return` and `break` out of a borrowed `if let`'s `Some` arm ends the
/// binding and the optional once; an exit from the `None` arm ends the optional at most once.
#[test]
fn borrowed_if_let_exits_end_its_temporary() {
    let src = "class T {\n    l: T?\n    v: i64\n}\n\
               fn g(t: T, c: bool) -> i64 {\n    let mut k = 0\n    while k < 3 {\n        if let x = t.l {\n            if c {\n                return x.v\n            }\n            break\n        } else {\n            if c {\n                continue\n            }\n            k += 1\n        }\n    }\n    k\n}";
    has(&built(src), "g", &["fn g(t#0: borrow T"]);
    let g = built_function(src, "g");
    let x = named(&g, "x");
    let optional = g
        .blocks
        .iter()
        .flat_map(|block| &block.stmts)
        .find_map(|stmt| match stmt {
            Statement::Assign {
                dest: Some(dest),
                value: Rvalue::Borrow(place),
            } if place.local == LocalId(0) => Some(*dest),
            _ => None,
        })
        .expect("`t.l` should be borrowed into a temporary");
    let exits = arm_exits(&g, &[x, optional]);
    let some = exits
        .iter()
        .filter(|&&block| ends(&g, block, x) > 0)
        .count();
    assert_eq!(
        some, 2,
        "a return and a break from the `Some` arm: {exits:?}"
    );
    for block in exits {
        if ends(&g, block, x) > 0 {
            assert_eq!(ends(&g, block, x), 1, "bb{}: {g:?}", block.0);
            assert_eq!(ends(&g, block, optional), 1, "bb{}: {g:?}", block.0);
        } else {
            assert!(ends(&g, block, optional) <= 1, "bb{}: {g:?}", block.0);
        }
    }
}

/// A borrowed `if let` whose arms both return has no join: each `return` ends the optional
/// once, and nothing else ends it.
#[test]
fn borrowed_if_let_with_no_join_ends_the_optional_at_each_return() {
    let src = "fn size(q: string?) -> i64 {\n    if let s = q {\n        return s.len()\n    } else {\n        return 0\n    }\n}";
    has(&built(src), "size", &["fn size(q#0: borrow string?)"]);
    let f = built_function(src, "size");
    let optional = f
        .blocks
        .iter()
        .flat_map(|block| &block.stmts)
        .find_map(|stmt| match stmt {
            Statement::Assign {
                dest: Some(dest),
                value: Rvalue::Borrow(place),
            } if place.local == LocalId(0) => Some(*dest),
            _ => None,
        })
        .expect("`q` should be borrowed into a temporary");
    let returns = blocks_where(&f, |term| matches!(term, Terminator::Return(_)));
    assert_eq!(returns.len(), 2, "{f:?}");
    for &block in &returns {
        assert_eq!(ends(&f, block, optional), 1, "bb{}: {f:?}", block.0);
    }
    let all = (0..f.blocks.len())
        .map(|b| ends(&f, BlockId(b), optional))
        .sum::<usize>();
    assert_eq!(all, 2, "{f:?}");
}
