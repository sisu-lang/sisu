//! The certifier: hand-built mir for each violation, the programs the spec names as accepted,
//! and the cost of a region.

use super::*;
use crate::mir::certify::{EDGES, Rule, Violation, certify};
use crate::source::Span;

// Statements and operands, written short.

fn whole(local: LocalId) -> Place {
    Place {
        local,
        projections: Vec::new(),
        borrow: false,
        guard: false,
    }
}

/// `(local as 0).0`, a `match` binding's place.
fn payload(local: LocalId) -> Place {
    Place {
        projections: vec![Projection::Payload {
            variant: 0,
            field: 0,
        }],
        ..whole(local)
    }
}

/// `local[0]`.
fn first(local: LocalId) -> Place {
    Place {
        projections: vec![Projection::Index {
            index: Operand::Const(Const::Int(0, Type::I64)),
            span: Span::new(0, 0),
        }],
        ..whole(local)
    }
}

fn read(local: LocalId) -> Operand {
    Operand::Read(whole(local))
}

fn assign(dest: LocalId, value: Rvalue) -> Statement {
    Statement::Assign {
        dest: Some(dest),
        value,
    }
}

/// `dest = call fresh()`: an owned value.
fn fresh(dest: LocalId) -> Statement {
    assign(
        dest,
        Rvalue::Call {
            func: FuncId(0),
            args: Vec::new(),
        },
    )
}

/// `dest = copy "x"`.
fn literal(dest: LocalId) -> Statement {
    assign(dest, Rvalue::CopyStr("x".to_owned()))
}

/// `call f(args)`, with a `unit` result.
fn call(args: Vec<Operand>) -> Statement {
    Statement::Assign {
        dest: None,
        value: Rvalue::Call {
            func: FuncId(0),
            args,
        },
    }
}

/// `print(op)`: a borrowed operand.
fn show(op: Operand) -> Statement {
    Statement::Assign {
        dest: None,
        value: Rvalue::Print {
            stream: Stream::Stdout,
            operand: op,
        },
    }
}

fn borrow(binding: LocalId, owner: LocalId) -> Statement {
    assign(binding, Rvalue::Borrow(payload(owner)))
}

/// The commit of a binding: `b = copy b`.
fn commit(binding: LocalId) -> Statement {
    assign(binding, Rvalue::Copy(whole(binding)))
}

fn goto(block: usize) -> Terminator {
    Terminator::Goto(BlockId(block))
}

fn branch(cond: LocalId, then: usize, else_: usize) -> Terminator {
    Terminator::Branch {
        cond: read(cond),
        then: BlockId(then),
        else_: BlockId(else_),
    }
}

fn ret() -> Terminator {
    Terminator::Return(None)
}

fn rejects(f: &Function, block: usize, local: LocalId, rule: Rule) {
    assert_eq!(
        certify(f),
        Err(Violation {
            block: BlockId(block),
            local,
            rule
        })
    );
}

/// Builds `src` and certifies each of its functions.
fn certifies(src: &str) -> Program {
    let program = build(&checked(src));
    for f in &program.functions {
        assert_eq!(certify(f), Ok(()), "fn {}", f.name);
    }
    program
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

// The spec's certifier tests.

#[test]
fn missing_release() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    b.block(vec![literal(s)], ret());
    rejects(&b.finish(), 0, s, Rule::Return);
}

#[test]
fn double_release() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    b.block(
        vec![literal(s), Statement::Release(s), Statement::Release(s)],
        ret(),
    );
    rejects(&b.finish(), 0, s, Rule::Consume);
}

#[test]
fn read_after_move() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    let t = b.local("t", Type::STRING);
    b.block(
        vec![
            literal(s),
            assign(t, Rvalue::Use(Operand::Move(s))),
            show(read(s)),
            Statement::Release(t),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, s, Rule::Read);
}

/// A `panic` waives only rule 4.
#[test]
fn read_after_move_before_a_panic() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    let m = b.local("m", Type::STRING);
    b.block(
        vec![literal(s), Statement::Release(s), show(read(s)), literal(m)],
        Terminator::Panic {
            message: Operand::Move(m),
            span: Span::new(0, 0),
        },
    );
    rejects(&b.finish(), 0, s, Rule::Read);
}

#[test]
fn borrowed_binding_outlives_its_scrutinee() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    let x = b.local("x", Type::STRING);
    b.owner(x, s);
    b.block(
        vec![
            fresh(s),
            borrow(x, s),
            Statement::Release(s),
            Statement::End(x),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, x, Rule::Borrow);
}

/// A value made in each iteration and never released reaches the loop head owned, and the next
/// assignment breaks rule 5.
#[test]
fn leak_on_a_loop_back_edge() {
    let mut b = Builder::new("f");
    let c = b.param("c", Type::BOOL);
    let s = b.local("s", Type::STRING);
    b.scope(s, BlockId(1));
    b.block(vec![], goto(1));
    b.block(vec![fresh(s)], branch(c, 1, 2));
    b.block(vec![Statement::Release(s)], ret());
    rejects(&b.finish(), 1, s, Rule::Assign);
}

#[test]
fn owned_and_empty_join_then_release() {
    let mut b = Builder::new("f");
    let c = b.param("c", Type::BOOL);
    let s = b.local("s", Type::STRING);
    b.block(vec![], branch(c, 1, 2));
    b.block(vec![fresh(s)], goto(3));
    b.block(vec![], goto(3));
    b.block(vec![Statement::Release(s)], ret());
    rejects(&b.finish(), 3, s, Rule::Consume);
}

#[test]
fn assignment_to_an_owned_local() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    b.block(vec![fresh(s), fresh(s), Statement::Release(s)], ret());
    rejects(&b.finish(), 0, s, Rule::Assign);
}

#[test]
fn copy_never_released() {
    let mut b = Builder::new("f");
    let p = b.param("p", Type::STRING);
    let t = b.local("t", Type::STRING);
    b.block(
        vec![assign(t, Rvalue::Copy(whole(p))), Statement::Release(p)],
        ret(),
    );
    rejects(&b.finish(), 0, t, Rule::Return);
}

/// `f(a[0], move a)`: the first operand reads `a` in place, and the second moves it.
#[test]
fn borrowed_operand_whose_owner_a_later_sibling_consumes() {
    let mut b = Builder::new("f");
    let a = b.param("a", Type::array(Type::I64));
    b.block(
        vec![call(vec![Operand::Read(first(a)), Operand::Move(a)])],
        ret(),
    );
    rejects(&b.finish(), 0, a, Rule::Borrow);
}

/// `f(s)` passes `s` without a copy or a move, so `f` and `s` would both own its reference.
#[test]
fn consuming_argument_read_in_place() {
    let mut b = Builder::new("f");
    let s = b.param("s", Type::STRING);
    b.block(vec![call(vec![read(s)]), Statement::Release(s)], ret());
    rejects(&b.finish(), 0, s, Rule::Consume);
}

/// `a[0] = s` stores `s` without a copy or a move.
#[test]
fn stored_value_read_in_place() {
    let mut b = Builder::new("f");
    let a = b.param("a", Type::array(Type::STRING));
    let s = b.param("s", Type::STRING);
    let old = b.local("old", Type::STRING);
    b.block(
        vec![
            Statement::Take {
                dest: old,
                place: first(a),
            },
            Statement::Store {
                place: first(a),
                value: read(s),
            },
            Statement::Release(old),
            Statement::Release(s),
            Statement::Release(a),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, s, Rule::Consume);
}

/// `a[0] = t` with the old element taken and never released.
#[test]
fn store_whose_taken_old_value_is_never_released() {
    let mut b = Builder::new("f");
    let a = b.param("a", Type::array(Type::STRING));
    let t = b.local("t", Type::STRING);
    let old = b.local("old", Type::STRING);
    b.block(
        vec![
            literal(t),
            Statement::Take {
                dest: old,
                place: first(a),
            },
            Statement::Store {
                place: first(a),
                value: Operand::Move(t),
            },
            Statement::Release(a),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, old, Rule::Return);
}

#[test]
fn take_with_no_store() {
    let mut b = Builder::new("f");
    let a = b.param("a", Type::array(Type::STRING));
    let old = b.local("old", Type::STRING);
    b.block(
        vec![
            Statement::Take {
                dest: old,
                place: first(a),
            },
            Statement::Release(old),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, a, Rule::Take);
}

#[test]
fn two_stores_after_one_take() {
    let mut b = Builder::new("f");
    let a = b.param("a", Type::array(Type::STRING));
    let t = b.local("t", Type::STRING);
    let u = b.local("u", Type::STRING);
    let old = b.local("old", Type::STRING);
    let store = |value| Statement::Store {
        place: first(a),
        value: Operand::Move(value),
    };
    b.block(
        vec![
            literal(t),
            literal(u),
            Statement::Take {
                dest: old,
                place: first(a),
            },
            store(t),
            store(u),
            Statement::Release(old),
            Statement::Release(a),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, a, Rule::Take);
}

/// The guard reads its bindings, and its `break` ends them before it leaves the loop.
#[test]
fn break_inside_a_guard() {
    let program = certifies(
        "enum P {\n    Two(string, string)\n    Zero\n}\n\
         fn g(ps: [P], c: bool) -> i64 {\n    for p in ps {\n        match p {\n            Two(x, y) if (if c { break } else { true }) => print(x)\n            _ => print(0)\n        }\n    }\n    0\n}",
    );
    let g = &program.functions[0];
    assert!(g.blocks.iter().any(|block| {
        block
            .stmts
            .iter()
            .any(|stmt| matches!(stmt, Statement::End(local) if *local == named(g, "x")))
    }));
}

/// The second alternative assigns `a` again, from the other field, while the first's borrow is
/// still in place: a retry keeps the binding borrowed.
#[test]
fn second_alternative_reassigns_its_binding() {
    let program = certifies(
        "enum Two {\n    Pair(string, string)\n    Single(string)\n}\n\
         fn keep(a: string) -> bool { a == \"x\" }\n\
         fn f(t: Two) -> string {\n    match t {\n        Pair(a, _) | Pair(_, a) if keep(a) => a\n        _ => \"\"\n    }\n}",
    );
    let f = &program.functions[1];
    let a = named(f, "a");
    let borrows = f
        .blocks
        .iter()
        .flat_map(|block| &block.stmts)
        .filter(|stmt| {
            matches!(stmt, Statement::Assign { dest: Some(dest), value: Rvalue::Borrow(_) } if *dest == a)
        })
        .count();
    assert_eq!(borrows, 2);
}

/// `order` from `tests/run/match_alternatives.sisu` with `string` fields, and an `|` of two
/// alternatives that bind one name from different fields.
#[test]
fn order_and_pair_alternatives() {
    let program = certifies(
        "enum Pair {\n    T(string, string)\n}\n\
         enum Ds {\n    D(Pair, Pair)\n    N\n}\n\
         fn see(a: string, b: string) -> bool { a == b }\n\
         fn order(d: Ds) -> string {\n    match d {\n        D(T(x, _) | T(_, x), T(y, _) | T(_, y)) if see(x, y) => \"seen\"\n        _ => \"none taken\"\n    }\n}\n\
         enum Two {\n    Pair(string, string)\n    Single(string)\n}\n\
         fn pick(t: Two) -> string {\n    match t {\n        Pair(a, _) | Pair(_, a) => a\n        Single(s) => s\n    }\n}",
    );
    let names: Vec<&str> = program.functions.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["see", "order", "pick", "main"]);
}

#[test]
fn read_of_a_binding_after_its_arm_fails() {
    let mut b = Builder::new("f");
    let c = b.param("c", Type::BOOL);
    let s = b.local("s", Type::STRING);
    let x = b.local("x", Type::STRING);
    b.owner(x, s);
    b.block(vec![fresh(s), borrow(x, s)], branch(c, 1, 2));
    b.block(
        vec![Statement::End(x), show(read(x)), Statement::Release(s)],
        ret(),
    );
    b.block(vec![Statement::End(x), Statement::Release(s)], ret());
    rejects(&b.finish(), 1, x, Rule::Read);
}

#[test]
fn scope_release_of_an_ended_binding() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    let x = b.local("x", Type::STRING);
    b.owner(x, s);
    b.block(
        vec![
            fresh(s),
            borrow(x, s),
            Statement::End(x),
            Statement::ScopeRelease(x),
            Statement::Release(s),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, x, Rule::Consume);
}

#[test]
fn read_between_take_and_store() {
    let mut b = Builder::new("f");
    let a = b.param("a", Type::array(Type::STRING));
    let t = b.local("t", Type::STRING);
    let old = b.local("old", Type::STRING);
    b.block(
        vec![
            literal(t),
            Statement::Take {
                dest: old,
                place: first(a),
            },
            show(read(a)),
            Statement::Store {
                place: first(a),
                value: Operand::Move(t),
            },
            Statement::Release(old),
            Statement::Release(a),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, a, Rule::Take);
}

/// `x` is borrowed on one path into `bb3` and empty on the other; `read` reads it there.
fn borrowed_or_empty(read_it: bool) -> (Function, LocalId) {
    let mut b = Builder::new("f");
    let c = b.param("c", Type::BOOL);
    let s = b.local("s", Type::STRING);
    let x = b.local("x", Type::STRING);
    b.owner(x, s);
    b.block(vec![fresh(s)], branch(c, 1, 2));
    b.block(vec![borrow(x, s)], goto(3));
    b.block(vec![], goto(3));
    let mut join = vec![Statement::End(x), Statement::Release(s)];
    if read_it {
        join.insert(0, show(read(x)));
    }
    b.block(join, ret());
    (b.finish(), x)
}

#[test]
fn borrowed_or_empty_then_read() {
    let (f, x) = borrowed_or_empty(true);
    rejects(&f, 3, x, Rule::Read);
}

#[test]
fn borrowed_or_empty_left_unread() {
    assert_eq!(certify(&borrowed_or_empty(false).0), Ok(()));
}

/// Three predecessors carry `x` borrowed, empty and owned into a `release? x`. `order[k]` is
/// where the predecessor of kind `k` sits among `bb1` to `bb3`. With `intermediate`, the
/// borrowed and the empty one meet in a join of their own first.
fn three_way_join(order: [usize; 3], intermediate: bool) -> (Function, BlockId, LocalId) {
    let mut b = Builder::new("f");
    let n = b.param("n", Type::I64);
    let s = b.local("s", Type::STRING);
    let x = b.local("x", Type::STRING);
    b.owner(x, s);
    b.block(
        vec![fresh(s)],
        Terminator::SwitchValue {
            operand: read(n),
            arms: vec![
                (Const::Int(0, Type::I64), BlockId(1)),
                (Const::Int(1, Type::I64), BlockId(2)),
            ],
            otherwise: Some(BlockId(3)),
        },
    );
    let (inner, outer) = if intermediate { (4, 5) } else { (4, 4) };
    for at in 0..3 {
        let kind = order
            .iter()
            .position(|&o| o == at)
            .expect("`order` should be a permutation");
        let (stmts, to) = match kind {
            0 => (vec![borrow(x, s)], inner),
            1 => (vec![], inner),
            _ => (vec![fresh(x)], outer),
        };
        b.block(stmts, goto(to));
    }
    if intermediate {
        b.block(vec![], goto(outer));
    }
    let join = b.block(
        vec![Statement::ScopeRelease(x), Statement::Release(s)],
        ret(),
    );
    (b.finish(), join, x)
}

/// The verdict at a join does not depend on the order of its predecessors, or on a join
/// between them.
#[test]
fn join_of_borrowed_empty_and_owned() {
    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    for intermediate in [false, true] {
        for order in orders {
            let (f, join, x) = three_way_join(order, intermediate);
            assert_eq!(
                certify(&f),
                Err(Violation {
                    block: join,
                    local: x,
                    rule: Rule::Consume
                }),
                "{order:?}, intermediate {intermediate}"
            );
        }
    }
}

/// `let x = a(); loop { read x }`: the loop head's back edge starts unreached, so `x` is owned
/// there and never maybe-empty.
#[test]
fn read_in_a_loop_of_a_local_assigned_before() {
    let mut b = Builder::new("f");
    let x = b.local("x", Type::STRING);
    b.block(vec![fresh(x)], goto(1));
    b.block(vec![show(read(x))], goto(1));
    assert_eq!(certify(&b.finish()), Ok(()));
}

/// `Two(x, 1)`: the pattern binds `x`, then its literal test fails, and the failure
/// edge ends `x`.
#[test]
fn pattern_fails_after_a_binding() {
    let program = certifies(
        "enum P {\n    Two(string, i64)\n    Zero\n}\n\
         fn f(p: P) -> string {\n    match p {\n        Two(x, 1) => x\n        _ => \"\"\n    }\n}",
    );
    assert_eq!(program.functions[0].name, "f");
}

#[test]
fn alternative_with_a_guard() {
    let program = certifies(
        "enum E {\n    S(string)\n    T(string)\n}\n\
         fn h(e: E, c: bool) -> i64 {\n    match e {\n        S(x) | T(x) if c => 1\n        _ => 0\n    }\n}",
    );
    assert_eq!(program.functions[0].name, "h");
}

/// `s` enters the loop owned; each iteration releases what it may hold, reassigns it and moves
/// it, so the head joins owned and empty and the `release?` accepts that.
#[test]
fn local_enters_a_loop_owned_then_is_reassigned_and_moved() {
    let mut b = Builder::new("f");
    let c = b.param("c", Type::BOOL);
    let s = b.local("s", Type::STRING);
    b.block(vec![fresh(s)], goto(1));
    b.block(vec![], branch(c, 2, 3));
    b.block(
        vec![
            Statement::ScopeRelease(s),
            fresh(s),
            call(vec![Operand::Move(s)]),
        ],
        goto(1),
    );
    b.block(vec![Statement::ScopeRelease(s)], ret());
    assert_eq!(certify(&b.finish()), Ok(()));
}

// The brief's own cases.

#[test]
fn borrow_not_ended_across_blocks() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    let x = b.local("x", Type::STRING);
    b.owner(x, s);
    b.block(vec![literal(s), borrow(x, s)], goto(1));
    b.block(vec![Statement::Release(s)], ret());
    rejects(&b.finish(), 1, x, Rule::Borrow);
}

/// `a[0] = 1` on an `[i64]`: an uncounted element holds no reference to take.
#[test]
fn scalar_store_needs_no_take() {
    let mut b = Builder::new("f");
    let a = b.param("a", Type::array(Type::I64));
    b.block(
        vec![
            Statement::Store {
                place: first(a),
                value: Operand::Const(Const::Int(1, Type::I64)),
            },
            Statement::Release(a),
        ],
        ret(),
    );
    assert_eq!(certify(&b.finish()), Ok(()));
}

/// The tag test fails before the arm assigns its binding, and the failure edge ends it.
#[test]
fn end_of_an_unassigned_binding() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    let x = b.local("x", Type::STRING);
    b.owner(x, s);
    b.block(
        vec![fresh(s)],
        Terminator::SwitchTag {
            place: whole(s),
            arms: vec![(0, BlockId(1))],
            otherwise: Some(BlockId(2)),
        },
    );
    b.block(
        vec![
            borrow(x, s),
            commit(x),
            Statement::ScopeRelease(s),
            Statement::Release(x),
        ],
        ret(),
    );
    b.block(vec![Statement::End(x), Statement::ScopeRelease(s)], ret());
    assert_eq!(certify(&b.finish()), Ok(()));
}

#[test]
fn end_of_an_owned_binding() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    let x = b.local("x", Type::STRING);
    b.owner(x, s);
    b.block(
        vec![
            fresh(s),
            borrow(x, s),
            commit(x),
            Statement::Release(s),
            Statement::End(x),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, x, Rule::End);
}

/// The `return` arm of Task 2's program, with the binding's `release?` taken off its edge.
#[test]
fn committed_binding_released_on_every_exit() {
    let src = include_str!("../../../tests/run/committed_binding_leaves_loop_once.sisu");
    let mut program = certifies(src);
    let walk = program
        .functions
        .iter_mut()
        .find(|f| f.name == "walk")
        .expect("the program should have `walk`");
    let c = named(walk, "c");
    let exit = walk
        .blocks
        .iter()
        .position(|block| {
            matches!(block.term, Terminator::Return(_))
                && block
                    .stmts
                    .iter()
                    .any(|stmt| matches!(stmt, Statement::ScopeRelease(local) if *local == c))
        })
        .expect("the `return` arm should release `c` on its way out");
    walk.blocks[exit]
        .stmts
        .retain(|stmt| !matches!(stmt, Statement::ScopeRelease(local) if *local == c));
    rejects(walk, exit, c, Rule::Return);
}

/// Hand-built, since a `while true` always has a `break` edge.
#[test]
fn leak_on_the_back_edge_of_an_endless_loop() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    b.scope(s, BlockId(1));
    b.block(vec![], goto(1));
    b.block(vec![fresh(s)], goto(1));
    b.add_loop(BlockId(0), BlockId(1), vec![BlockId(1)]);
    rejects(&b.finish(), 1, s, Rule::Assign);
}

/// `bb0: branch c bb1 bb2`, `bb1: s = call fresh(); goto bb2`, `bb2: goto bb0`, with `s`
/// scoped to `bb1`: it leaves its scope owned.
fn branch_of_an_endless_loop(recorded: bool) -> (Function, LocalId) {
    let mut b = Builder::new("f");
    let c = b.param("c", Type::BOOL);
    let s = b.local("s", Type::STRING);
    b.scope(s, BlockId(1));
    b.block(vec![], branch(c, 1, 2));
    b.block(vec![fresh(s)], goto(2));
    b.block(vec![], goto(0));
    if recorded {
        // The entry falls into the loop, so it is its own pre-header.
        b.add_loop(
            BlockId(0),
            BlockId(0),
            vec![BlockId(0), BlockId(1), BlockId(2)],
        );
    }
    (b.finish(), s)
}

#[test]
fn leak_from_a_branch_of_an_endless_loop() {
    let (f, s) = branch_of_an_endless_loop(true);
    rejects(&f, 1, s, Rule::Escape);
}

/// The certifier finds the cycle by itself: `Function::loops` serves only the hoist.
#[test]
fn leak_round_an_unrecorded_cycle() {
    let (f, s) = branch_of_an_endless_loop(false);
    assert!(f.loops.is_empty());
    rejects(&f, 1, s, Rule::Escape);
}

/// The spec's `unwrap`: the `Some` edge moves the payload out, and the `None` edge leaves the
/// optional empty.
#[test]
fn ifsome_certifies() {
    let program =
        certifies("fn unwrap(p: string?) -> string { if let q = p { q } else { \"none\" } }");
    let unwrap = &program.functions[0];
    let presence = unwrap
        .blocks
        .iter()
        .filter(|block| matches!(block.term, Terminator::IfPresent { .. }))
        .count();
    assert_eq!(presence, 1);
}

#[test]
fn unreachable_code_certifies() {
    let program = certifies(UNREACHABLE_CODE);
    assert_eq!(program.functions.len(), 6);
}

/// `owners[b] = s`, `owners[c] = b`: `c`'s root is `s`. `releases_root` releases `s` while
/// `c` is still borrowed.
fn borrow_chain(releases_root: bool) -> (Function, LocalId) {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    let x = b.local("b", Type::STRING);
    let c = b.local("c", Type::STRING);
    b.owner(x, s);
    b.owner(c, x);
    let ends = if releases_root {
        [Statement::End(x), Statement::Release(s), Statement::End(c)]
    } else {
        [Statement::End(c), Statement::End(x), Statement::Release(s)]
    };
    let mut stmts = vec![fresh(s), borrow(x, s), borrow(c, x)];
    stmts.extend(ends);
    b.block(stmts, ret());
    (b.finish(), c)
}

#[test]
fn borrow_chain_root_released() {
    let (f, c) = borrow_chain(true);
    rejects(&f, 0, c, Rule::Borrow);
}

#[test]
fn borrow_chain_root_live() {
    assert_eq!(certify(&borrow_chain(false).0), Ok(()));
}

// One case for each check the cases above leave alone.

/// `s` is scoped to `bb1`, which does not dominate `bb2`, where `s` is used.
#[test]
fn mention_outside_its_scope() {
    let mut b = Builder::new("f");
    let c = b.param("c", Type::BOOL);
    let s = b.local("s", Type::STRING);
    b.scope(s, BlockId(1));
    b.block(vec![], branch(c, 1, 2));
    b.block(vec![], goto(2));
    b.block(vec![fresh(s), Statement::Release(s)], ret());
    rejects(&b.finish(), 2, s, Rule::Scope);
}

/// `s` is released after `x`'s last effect in the block, while `x` still borrows from it.
#[test]
fn scrutinee_released_after_its_binding_borrows() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    let x = b.local("x", Type::STRING);
    b.owner(x, s);
    b.block(vec![fresh(s), borrow(x, s), Statement::Release(s)], goto(1));
    b.block(vec![Statement::End(x)], ret());
    rejects(&b.finish(), 0, x, Rule::Borrow);
}

/// A committed binding borrowed again, which drops the reference it owns.
#[test]
fn borrow_into_an_owned_binding() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    let x = b.local("x", Type::STRING);
    b.owner(x, s);
    b.block(
        vec![
            fresh(s),
            borrow(x, s),
            commit(x),
            borrow(x, s),
            Statement::End(x),
            Statement::Release(s),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, x, Rule::Assign);
}

/// `x` is declared before `s`, so it is checked first; `s` leaks too, but only `x` is
/// still borrowed at the `return`.
#[test]
fn borrow_reaches_a_return() {
    let mut b = Builder::new("f");
    let x = b.local("x", Type::STRING);
    let s = b.local("s", Type::STRING);
    b.owner(x, s);
    b.block(vec![fresh(s), borrow(x, s)], ret());
    rejects(&b.finish(), 0, x, Rule::Unended);
}

/// `x` is scoped to `bb1` and leaves it borrowed.
#[test]
fn borrow_leaves_its_scope() {
    let mut b = Builder::new("f");
    let c = b.param("c", Type::BOOL);
    let x = b.local("x", Type::STRING);
    let s = b.local("s", Type::STRING);
    b.owner(x, s);
    b.scope(x, BlockId(1));
    b.block(vec![fresh(s)], branch(c, 1, 2));
    b.block(vec![borrow(x, s)], goto(2));
    b.block(vec![Statement::Release(s)], ret());
    rejects(&b.finish(), 1, x, Rule::Unended);
}

/// `bb1` mentions no `s`, so `bb0`'s set covers it, and its `return` finds `s` owned.
#[test]
fn return_on_a_branch_that_skips_the_release() {
    let mut b = Builder::new("f");
    let c = b.param("c", Type::BOOL);
    let s = b.local("s", Type::STRING);
    b.block(vec![fresh(s)], branch(c, 1, 2));
    b.block(vec![], ret());
    b.block(vec![Statement::Release(s)], ret());
    rejects(&b.finish(), 1, s, Rule::Return);
}

/// `s` is scoped to `bb1`; `bb2` mentions no `s`, so `bb1`'s set covers it, and its edge to
/// `bb4` leaves the scope with `s` owned.
#[test]
fn exit_from_a_block_that_skips_the_release() {
    let mut b = Builder::new("f");
    let c = b.param("c", Type::BOOL);
    let s = b.local("s", Type::STRING);
    b.scope(s, BlockId(1));
    b.block(vec![], branch(c, 1, 4));
    b.block(vec![fresh(s)], branch(c, 2, 3));
    b.block(vec![], goto(4));
    b.block(vec![Statement::Release(s)], goto(4));
    b.block(vec![], ret());
    rejects(&b.finish(), 2, s, Rule::Escape);
}

/// The scope release of each `s_i` sits in the function's last block, after every later `if`.
/// Each `s_i`'s region is its own block and that last one, so the edges the certifier reads
/// grow by the same amount for each `n` more locals.
#[test]
fn scope_release_gap_is_sparse() {
    let source = |n: usize| {
        let body = (0..n)
            .map(|i| {
                format!("    let s_{i} = \"x\"\n    print(s_{i})\n    if c {{ print({i}) }}\n")
            })
            .collect::<Vec<_>>()
            .concat();
        format!("fn f(c: bool) {{\n{body}}}")
    };
    let edges = |n: usize| {
        let program = build(&checked(&source(n)));
        let f = &program.functions[0];
        let locals = f
            .locals
            .iter()
            .filter(|local| {
                local
                    .name
                    .as_deref()
                    .is_some_and(|name| name.starts_with("s_"))
            })
            .count();
        assert_eq!(locals, n);
        EDGES.set(0);
        assert_eq!(certify(f), Ok(()));
        EDGES.get()
    };
    let n = 100;
    let (one, two, three) = (edges(n), edges(2 * n), edges(3 * n));
    assert!(one > 0);
    assert_eq!(
        three - two,
        two - one,
        "{one}, {two}, {three} edges for {n}, {}, {}",
        2 * n,
        3 * n
    );
}

// The ported timeout tests, now through lowering, the ownership pass and the certifier. Their
// keep-alive overrides are in `.config/nextest.toml`.

/// `match e` over `variants` variants, each arm `V{i}(s) {guard} => s`, with `head` before it.
fn wide_match(
    variants: usize,
    params: &str,
    head: &str,
    guard: impl Fn(usize) -> String,
) -> String {
    let variant_list = (0..variants)
        .map(|i| format!("    V{i}(string)\n"))
        .collect::<Vec<_>>()
        .concat();
    let arms = (0..variants)
        .map(|i| format!("        V{i}(s){} => s\n", guard(i)))
        .collect::<Vec<_>>()
        .concat();
    format!(
        "enum E {{\n{variant_list}}}\nfn f(e: E{params}) -> string {{\n{head}    match e {{\n{arms}        _ => \"\"\n    }}\n}}"
    )
}

/// How often the printed mir of `src` moves an `s`.
fn moves_of_s(src: &str) -> usize {
    let program = checked(src);
    print(&build(&program), &program).matches("move s#").count()
}

#[test]
fn many_guarded_arms_mark_last_uses_in_linear_time() {
    const VARIANTS: usize = 32_000;
    let src = wide_match(VARIANTS, ", keep: bool", "", |_| " if keep".to_owned());
    // Each arm's `s` is read once, in its body, so every read is its last use.
    assert_eq!(moves_of_s(&src), VARIANTS);
}

#[test]
fn many_guards_of_distinct_locals_mark_last_uses_in_linear_time() {
    const VARIANTS: usize = 48_000;
    let locals = (0..VARIANTS)
        .map(|i| format!("    let g{i} = c\n"))
        .collect::<Vec<_>>()
        .concat();
    let src = wide_match(VARIANTS, ", c: bool", &locals, |i| format!(" if g{i}"));
    assert_eq!(moves_of_s(&src), VARIANTS);
}

#[test]
fn nested_guards_mark_last_uses_in_linear_time() {
    const DEPTH: usize = 48;
    let guard = (0..DEPTH).fold("c".to_owned(), |inner, _| {
        format!("match c {{\n true if {inner} => true\n _ => false\n }}")
    });
    let src = format!(
        "enum E {{\n    V(string)\n    W\n}}\nfn f(e: E, c: bool) -> string {{\n    match e {{\n        V(s) if {guard} => s\n        _ => \"\"\n    }}\n}}"
    );
    assert_eq!(moves_of_s(&src), 1);
}

/// Arm `i`'s test hangs off arm `i - 1`'s failure edge, and the join has one predecessor per
/// arm: iterative dominators are quadratic here.
#[test]
fn many_arms_mark_last_uses_in_linear_time() {
    const VARIANTS: usize = 200_000;
    let variants = (0..VARIANTS)
        .map(|i| format!("    V{i}(string)\n"))
        .collect::<Vec<_>>()
        .concat();
    let arms = (0..VARIANTS)
        .map(|i| format!("        V{i}(s) => s\n"))
        .collect::<Vec<_>>()
        .concat();
    let src = format!(
        "enum E {{\n{variants}}}\nfn f(e: E) -> string {{\n    match e {{\n{arms}    }}\n}}"
    );
    // Each arm's `s` is read once, so every read is its last use.
    assert_eq!(moves_of_s(&src), VARIANTS);
}
