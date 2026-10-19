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

/// The `switch` on `tried` that ends `block`, if one does: its arms and its otherwise.
fn switch_on(f: &Function, block: BlockId, tried: LocalId) -> Option<(Vec<BlockId>, BlockId)> {
    if let Terminator::SwitchValue {
        operand: Operand::Read(place),
        arms,
        otherwise: Some(otherwise),
    } = &f.blocks[block.0].term
        && place.local == tried
    {
        Some((arms.iter().map(|(_, to)| *to).collect(), *otherwise))
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
        "fn unwrap(p: string?) -> string { if let q = p { q } else { return \"none\" } }\n\
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
            "_ = use p#0",
            "if_present _ bb1 bb2",
            "bb1:",
            "q#1 = unwrap move _",
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
        "fn f(n: i64, b: u8, s: string, c: char) -> i64 {\n\
             let x = match n {\n        3 => 1\n        _ => 0\n    }\n\
             let y = match b {\n        7 => 1\n        _ => 0\n    }\n\
             let z = match s {\n        \"hi\" => 1\n        _ => 0\n    }\n\
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
         fn f(b: Box, c: bool) -> string {\n    match b {\n        Full(x) => {\n            if c {\n                return x\n            }\n            x\n        }\n        Empty => \"\"\n    }\n}";
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
        let fields: Vec<_> = starts
            .iter()
            .map(|&start| borrowed_field(f, start))
            .collect();
        assert_eq!(fields, [0, 1], "`T(x, _)` is tried before `T(_, x)`");
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
         fn f(w: W) -> i64 {{\n    match w {{\n        V({}) if {} => 1\n        _ => 0\n    }}\n}}",
        vec!["string"; wide].join(", "),
        names.join(", "),
        reads.join(" && "),
    );
    let program = checked(&src);
    PROBED.set(0);
    let mir = lower(&program);
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

/// Inside an arm's guard, a use of a binding is marked, and so is a borrow site rooted at one;
/// in the body, after the commit, neither is.
#[test]
fn guard_uses_of_bindings_are_marked() {
    let program = lowered_program(
        "enum E {\n    S(string)\n    U\n}\n\
         fn keep(s: string) -> bool { true }\n\
         fn f(e: E) -> i64 {\n    match e {\n        S(x) if keep(x) && x.len() > 0 => x.len()\n        _ => 0\n    }\n}",
    );
    let f = func(&program, "f");
    let x = named(f, "x");
    let (mut guarded, mut used, mut sites) = (0, 0, Vec::new());
    for block in &f.blocks {
        for stmt in &block.stmts {
            if let Statement::Assign {
                value: Rvalue::Call { args, .. },
                ..
            } = stmt
            {
                guarded += args.iter().filter(|a| **a == Operand::Guarded(x)).count();
                used += args.iter().filter(|a| **a == Operand::Use(x)).count();
            }
            if let Statement::Assign {
                value: Rvalue::Len(Operand::Read(place)),
                ..
            } = stmt
                && place.local == x
            {
                sites.push((place.borrow, place.guard));
            }
        }
    }
    assert_eq!((guarded, used), (1, 0));
    assert_eq!(sites, [(true, true), (true, false)]);
}
