//! The certifier: hand-built mir for each violation, the programs the spec names as accepted,
//! and the cost of a region.

use super::*;
use crate::mir::certify::range::RangeMin;
use crate::mir::certify::wavelet::Wavelet;
use crate::mir::certify::{READS, Rule, Violation, certify};
use crate::source::Span;
use crate::tir::TypeKind;

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

/// How many statements of `f` borrow into `binding`.
fn borrows(f: &Function, binding: LocalId) -> usize {
    f.blocks
        .iter()
        .flat_map(|block| &block.stmts)
        .filter(|stmt| {
            matches!(stmt, Statement::Assign { dest: Some(dest), value: Rvalue::Borrow(_) } if *dest == binding)
        })
        .count()
}

/// Whether block `at` of `f` starts with `end binding`.
fn starts_with_end(f: &Function, at: BlockId, binding: LocalId) -> bool {
    matches!(f.blocks[at.0].stmts.first(), Some(Statement::End(local)) if *local == binding)
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
    assert_eq!(borrows(f, named(f, "a")), 2);
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
    // Each name is bound once per alternative that binds it.
    let order = &program.functions[1];
    assert_eq!(borrows(order, named(order, "x")), 2);
    assert_eq!(borrows(order, named(order, "y")), 2);
    let pick = &program.functions[2];
    assert_eq!(borrows(pick, named(pick, "a")), 2);
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

/// `old = take a[i]; i = 1; a[i] = move t`: the store would fill an element other than the
/// one taken.
#[test]
fn index_changed_between_take_and_store() {
    let mut b = Builder::new("f");
    let a = b.param("a", Type::array(Type::STRING));
    let i = b.param("i", Type::I64);
    let t = b.local("t", Type::STRING);
    let old = b.local("old", Type::STRING);
    let at_i = Place {
        projections: vec![Projection::Index {
            index: read(i),
            span: Span::new(0, 0),
        }],
        ..whole(a)
    };
    b.block(
        vec![
            literal(t),
            Statement::Take {
                dest: old,
                place: at_i.clone(),
            },
            assign(i, Rvalue::Use(Operand::Const(Const::Int(1, Type::I64)))),
            Statement::Store {
                place: at_i,
                value: Operand::Move(t),
            },
            Statement::Release(old),
            Statement::Release(a),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, i, Rule::Take);
}

/// `old = take a[k[0]]; k[0] = 1; a[k[0]] = move t`: a store into the index's array moves the
/// address too.
#[test]
fn index_array_stored_between_take_and_store() {
    let mut b = Builder::new("f");
    let a = b.param("a", Type::array(Type::STRING));
    let k = b.param("k", Type::array(Type::I64));
    let t = b.local("t", Type::STRING);
    let old = b.local("old", Type::STRING);
    let at_k = Place {
        projections: vec![Projection::Index {
            index: Operand::Read(first(k)),
            span: Span::new(0, 0),
        }],
        ..whole(a)
    };
    b.block(
        vec![
            literal(t),
            Statement::Take {
                dest: old,
                place: at_k.clone(),
            },
            Statement::Store {
                place: first(k),
                value: Operand::Const(Const::Int(1, Type::I64)),
            },
            Statement::Store {
                place: at_k,
                value: Operand::Move(t),
            },
            Statement::Release(old),
            Statement::Release(k),
            Statement::Release(a),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, k, Rule::Take);
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
    let f = &program.functions[0];
    let x = named(f, "x");
    // The block that binds `x` tests the literal, and its failure edge ends `x`.
    assert!(f.blocks.iter().any(|block| {
        block.stmts.iter().any(|stmt| {
            matches!(stmt, Statement::Assign { dest: Some(dest), value: Rvalue::Borrow(_) } if *dest == x)
        }) && matches!(block.term, Terminator::SwitchValue { otherwise: Some(to), .. } if starts_with_end(f, to, x))
    }));
}

#[test]
fn alternative_with_a_guard() {
    let program = certifies(
        "enum E {\n    S(string)\n    T(string)\n}\n\
         fn h(e: E, c: bool) -> i64 {\n    match e {\n        S(x) | T(x) if c => 1\n        _ => 0\n    }\n}",
    );
    let h = &program.functions[0];
    let x = named(h, "x");
    assert_eq!(borrows(h, x), 2);
    // The guard's failure edge retries the next alternative, and after the last it ends `x`.
    assert!(h.blocks.iter().any(|block| {
        matches!(block.term, Terminator::Branch { else_, .. } if matches!(
            h.blocks[else_.0].term,
            Terminator::SwitchValue { otherwise: Some(to), .. } if starts_with_end(h, to, x)
        ))
    }));
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

/// `a[0] = None` on a `[string?]` overwrites the element's reference, so it needs a `take`.
#[test]
fn none_stored_with_no_take() {
    let mut b = Builder::new("f");
    let a = b.param("a", Type::array(Type::optional(Type::STRING)));
    b.block(
        vec![
            Statement::Store {
                place: first(a),
                value: Operand::Const(Const::None(Type::optional(Type::STRING))),
            },
            Statement::Release(a),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, a, Rule::Take);
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

// The certifier's contracts, one test each.

/// `bb3` dominates neither `bb2`'s path through `bb1` nor `bb2`, so `s`, scoped at `bb3`, may not
/// be mentioned in `bb2`. Lengauer and Tarjan's relative dominator decides `bb2`'s here.
#[test]
fn dominators_of_intersecting_paths() {
    let mut b = Builder::new("f");
    let c = b.param("c", Type::BOOL);
    let s = b.local("s", Type::STRING);
    b.scope(s, BlockId(3));
    b.block(vec![], branch(c, 3, 1));
    b.block(vec![], goto(2));
    b.block(vec![fresh(s), Statement::Release(s)], ret());
    b.block(vec![], branch(c, 1, 2));
    rejects(&b.finish(), 2, s, Rule::Scope);
}

/// `old = take a[0]; a[0][1] = 5`: a store inside the taken element.
#[test]
fn store_inside_a_taken_place() {
    let mut b = Builder::new("f");
    let a = b.param("a", Type::array(Type::array(Type::I64)));
    let t = b.local("t", Type::array(Type::I64));
    let old = b.local("old", Type::array(Type::I64));
    let mut inner = first(a);
    inner.projections.push(Projection::Index {
        index: Operand::Const(Const::Int(1, Type::I64)),
        span: Span::new(0, 0),
    });
    b.block(
        vec![
            fresh(t),
            Statement::Take {
                dest: old,
                place: first(a),
            },
            Statement::Store {
                place: inner,
                value: Operand::Const(Const::Int(5, Type::I64)),
            },
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

/// `old = take o.0; u = copy o.1`: two fields of one object do not overlap.
#[test]
fn read_of_a_field_beside_a_taken_one() {
    let mut b = Builder::new("f");
    let o = b.param("o", Type::intern(TypeKind::Class(ClassId(0))));
    let t = b.local("t", Type::STRING);
    let u = b.local("u", Type::STRING);
    let old = b.local("old", Type::STRING);
    let field = |at| Place {
        projections: vec![Projection::Field(at)],
        ..whole(o)
    };
    b.block(
        vec![
            literal(t),
            Statement::Take {
                dest: old,
                place: field(0),
            },
            assign(u, Rvalue::Copy(field(1))),
            Statement::Store {
                place: field(0),
                value: Operand::Move(t),
            },
            Statement::Release(old),
            Statement::Release(u),
            Statement::Release(o),
        ],
        ret(),
    );
    assert_eq!(certify(&b.finish()), Ok(()));
}

/// A parameter is owned from the start, so one never mentioned still owes its release.
#[test]
fn unmentioned_parameter_reaches_a_return() {
    let mut b = Builder::new("f");
    let s = b.param("s", Type::STRING);
    b.block(vec![], ret());
    rejects(&b.finish(), 0, s, Rule::Return);
}

/// A local lowering never reached, in dead code after a `return`, is empty throughout.
#[test]
fn unmentioned_local_is_ignored() {
    let mut b = Builder::new("f");
    b.local("s", Type::STRING);
    b.block(vec![], ret());
    assert_eq!(certify(&b.finish()), Ok(()));
}

/// Sol's case: `x` leaves `bb2` ended, and no path comes back to `bb2`. With `covered`, it
/// leaves from `bb4`, a block of `bb2`'s that does not mention `x`.
#[test]
fn ended_binding_leaves_and_never_comes_back() {
    for covered in [false, true] {
        let mut b = Builder::new("f");
        let c = b.param("c", Type::BOOL);
        let s = b.param("s", Type::STRING);
        let x = b.local("x", Type::STRING);
        b.scope(x, BlockId(2));
        b.owner(x, s);
        b.block(vec![], goto(1));
        b.block(vec![], branch(c, 2, 3));
        b.block(
            vec![Statement::ScopeRelease(x), borrow(x, s), Statement::End(x)],
            goto(if covered { 4 } else { 3 }),
        );
        b.block(vec![Statement::Release(s)], ret());
        b.block(vec![], goto(3));
        assert_eq!(certify(&b.finish()), Ok(()), "covered {covered}");
    }
}

/// `bb4`, below `x`'s scope block `bb3`, joins `bb2`, a block as deep as `bb3` beside it and
/// before it in the dominator tree's pre-order. `bb2` stays out of the region, so `bb3`, first
/// in it, still takes the empty `x` from outside.
#[test]
fn join_beside_the_scope_block_stays_outside() {
    let mut b = Builder::new("f");
    let c = b.param("c", Type::BOOL);
    let x = b.local("x", Type::STRING);
    b.scope(x, BlockId(3));
    b.block(vec![], goto(1));
    b.block(vec![], branch(c, 3, 2));
    b.block(vec![], ret());
    b.block(vec![show(read(x))], goto(4));
    b.block(vec![show(read(x))], goto(2));
    rejects(&b.finish(), 3, x, Rule::Read);
}

/// A loop whose body `bb2` borrows into `x` and ends it. With `release`, `bb2` also scope-releases
/// `x` first, which the ended value that comes back round the loop breaks. With `covered`, the
/// back edge leaves from `bb4`, a block of `bb2`'s that does not mention `x`.
fn ended_binding_comes_back(release: bool, covered: bool) -> (Function, LocalId) {
    let mut b = Builder::new("f");
    let c = b.param("c", Type::BOOL);
    let s = b.param("s", Type::STRING);
    let x = b.local("x", Type::STRING);
    b.scope(x, BlockId(2));
    b.owner(x, s);
    b.block(vec![], goto(1));
    b.block(vec![], branch(c, 2, 3));
    let mut body = vec![borrow(x, s), Statement::End(x)];
    if release {
        body.insert(0, Statement::ScopeRelease(x));
    }
    b.block(body, goto(if covered { 4 } else { 1 }));
    b.block(vec![Statement::Release(s)], ret());
    b.block(vec![], goto(1));
    (b.finish(), x)
}

#[test]
fn ended_binding_comes_back_to_a_scope_release() {
    for covered in [false, true] {
        let (f, x) = ended_binding_comes_back(true, covered);
        assert_eq!(
            certify(&f),
            Err(Violation {
                block: BlockId(2),
                local: x,
                rule: Rule::Consume
            }),
            "covered {covered}"
        );
    }
}

#[test]
fn ended_binding_comes_back_to_a_borrow() {
    for covered in [false, true] {
        assert_eq!(
            certify(&ended_binding_comes_back(false, covered).0),
            Ok(()),
            "covered {covered}"
        );
    }
}

/// `o` is empty on the `None` edge, so the read there is a use after consume.
#[test]
fn read_on_the_none_edge() {
    let mut b = Builder::new("f");
    let o = b.local("o", Type::optional(Type::STRING));
    b.block(
        vec![fresh(o)],
        Terminator::IfPresent {
            optional: o,
            some: BlockId(1),
            none: BlockId(2),
        },
    );
    b.block(vec![Statement::Release(o)], ret());
    b.block(vec![show(read(o))], ret());
    rejects(&b.finish(), 2, o, Rule::Read);
}

/// `o` keeps its value on the `Some` edge, so a `return` there leaks it.
#[test]
fn leak_on_the_some_edge() {
    let mut b = Builder::new("f");
    let o = b.local("o", Type::optional(Type::STRING));
    b.block(
        vec![fresh(o)],
        Terminator::IfPresent {
            optional: o,
            some: BlockId(1),
            none: BlockId(2),
        },
    );
    b.block(vec![show(read(o))], ret());
    b.block(vec![], ret());
    rejects(&b.finish(), 1, o, Rule::Return);
}

/// `release x` of a borrowed binding, and of an ended one: neither owns a reference.
#[test]
fn release_of_a_borrowed_or_ended_binding() {
    for ended in [false, true] {
        let mut b = Builder::new("f");
        let s = b.local("s", Type::STRING);
        let x = b.local("x", Type::STRING);
        b.owner(x, s);
        let mut stmts = vec![fresh(s), borrow(x, s), Statement::Release(x)];
        if ended {
            stmts.insert(2, Statement::End(x));
        }
        stmts.push(Statement::Release(s));
        b.block(stmts, ret());
        assert_eq!(
            certify(&b.finish()),
            Err(Violation {
                block: BlockId(0),
                local: x,
                rule: Rule::Consume
            }),
            "ended {ended}"
        );
    }
}

/// `bb1` is unreachable, so its double release is never run.
#[test]
fn dead_block_is_ignored() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    b.block(vec![fresh(s), Statement::Release(s)], ret());
    b.block(vec![Statement::Release(s), Statement::Release(s)], ret());
    assert_eq!(certify(&b.finish()), Ok(()));
}

/// The presence branch in `bb0` makes `bb2` a node before `bb1`'s events and `bb2`'s own are
/// seen; `bb2` still applies only its own.
#[test]
fn none_target_applies_its_own_events() {
    let mut b = Builder::new("f");
    let o = b.param("o", Type::optional(Type::STRING));
    b.block(
        vec![],
        Terminator::IfPresent {
            optional: o,
            some: BlockId(1),
            none: BlockId(2),
        },
    );
    b.block(vec![Statement::Release(o)], ret());
    b.block(vec![fresh(o), Statement::Release(o)], ret());
    assert_eq!(certify(&b.finish()), Ok(()));
}

/// `bb2` and `bb3` hold `s` owned without mentioning it; only `bb3` leaves `s`'s region.
#[test]
fn escape_names_the_block_that_leaves() {
    let mut b = Builder::new("f");
    let c = b.param("c", Type::BOOL);
    let s = b.local("s", Type::STRING);
    b.scope(s, BlockId(1));
    b.block(vec![], branch(c, 1, 4));
    b.block(vec![fresh(s)], goto(2));
    b.block(vec![], goto(3));
    b.block(vec![], goto(4));
    b.block(vec![], ret());
    rejects(&b.finish(), 3, s, Rule::Escape);
}

/// At a `panic` the program ends, so a value still owned there is no leak.
#[test]
fn owned_value_at_a_panic() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    let m = b.local("m", Type::STRING);
    b.block(
        vec![fresh(s), literal(m)],
        Terminator::Panic {
            message: Operand::Move(m),
            span: Span::new(0, 0),
        },
    );
    assert_eq!(certify(&b.finish()), Ok(()));
}

/// Only rule 4 is waived at a `panic`: a double release before one is still an error.
#[test]
fn double_release_before_a_panic() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    let m = b.local("m", Type::STRING);
    b.block(
        vec![
            fresh(s),
            Statement::Release(s),
            Statement::Release(s),
            literal(m),
        ],
        Terminator::Panic {
            message: Operand::Move(m),
            span: Span::new(0, 0),
        },
    );
    rejects(&b.finish(), 0, s, Rule::Consume);
}

/// A debug build panics on the first violation, naming the function, the block and the local.
#[test]
#[should_panic(expected = "mir certifier: fn f, bb0, s#0: may still own a reference at `return`")]
fn violation_panics_in_a_debug_build() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    b.block(vec![fresh(s)], ret());
    certified(&b.finish());
}

/// Every range of a pseudo-random array, and of a falling one, against a scan: its least
/// value, and the positions whose values lie in a range.
#[test]
fn range_queries_match_a_scan() {
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let values: Vec<u32> = (0..300)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            u32::try_from(seed % 40).expect("a value under 40 should fit")
        })
        .collect();
    // Falling values put each range's least at its end, where a query that reads past the
    // range finds a lesser one.
    for values in [values.clone(), (0..600).rev().collect()] {
        let least = RangeMin::new(values.clone());
        for from in 0..values.len() {
            for to in from..values.len() {
                let at = least.argmin(from, to);
                assert!((from..=to).contains(&at), "{from}..={to}");
                assert_eq!(least.min(from, to), values[at], "{from}..={to}");
                assert_eq!(
                    Some(&values[at]),
                    values[from..=to].iter().min(),
                    "{from}..={to}"
                );
            }
        }
    }
    let wavelet = Wavelet::new(&values, &(0..values.len()).collect::<Vec<_>>());
    let mut stack = Vec::new();
    for (low, high) in [(0, 39), (5, 9), (12, 12), (39, 50), (3, 2)] {
        for from in (0..=values.len()).step_by(7) {
            for to in (from..=values.len()).step_by(5) {
                let mut found = Vec::new();
                wavelet.report(from, to, low, high, &mut stack, |at| found.push(at));
                found.sort_unstable();
                let expected: Vec<usize> = (from..to)
                    .filter(|&at| (low..=high).contains(&values[at]))
                    .collect();
                assert_eq!(found, expected, "{from}..{to} in {low}..={high}");
            }
        }
    }
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
        READS.set(0);
        assert_eq!(certify(f), Ok(()));
        READS.get()
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

/// `while D_n {} print(s)`, with `D_0 = c` and `D_{n+1} = { while D_n {} c }`: block `k` of
/// the nesting has every loop head around it in its dominance frontier, so storing every
/// block's frontier grows as `n^2`. The reads grow by the same amount for each 10 more levels.
#[test]
fn nested_loop_conditions_read_linearly() {
    let source = |n: usize| {
        let mut cond = "c".to_owned();
        for _ in 0..n {
            cond = format!("{{\n        while {cond} {{}}\n        c\n    }}");
        }
        format!("fn f(c: bool, s: string) {{\n    while {cond} {{}}\n    print(s)\n}}")
    };
    let reads = |n: usize| {
        let program = build(&checked(&source(n)));
        let f = &program.functions[0];
        assert_eq!(f.loops.len(), n + 1);
        READS.set(0);
        assert_eq!(certify(f), Ok(()));
        READS.get()
    };
    let (one, two, three) = (reads(10), reads(20), reads(30));
    assert!(one > 0);
    assert_eq!(three - two, two - one, "{one}, {two}, {three} reads");
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
