//! The certifier: hand-built mir for each violation, the programs the spec names as accepted,
//! and the cost of a region.

use super::*;
use crate::mir::certify::range::RangeMin;
use crate::mir::certify::wavelet::Wavelet;
use crate::mir::certify::{
    EMPTY, Effect, OWNED, READS, Rule, Violation, certify, scope_release_states, statement,
};
use crate::mir::elaborate::flagged;
use crate::source::Span;
use crate::tir::{TypeArgs, TypeKind};

// Statements and operands, written short.

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

/// `call f(args)`, with a `unit` result, and `borrowed` the argument flags.
fn call(args: Vec<Operand>, borrowed: Vec<bool>) -> Statement {
    Statement::Assign {
        dest: None,
        value: Rvalue::Call {
            func: FuncId(0),
            args,
            borrowed,
        },
    }
}

/// `print(op)`: a borrowed operand, whose helper calls a written `to_string` when `calls`.
fn printing(op: Operand, calls: bool) -> Statement {
    Statement::Assign {
        dest: None,
        value: Rvalue::Print {
            stream: Stream::Stdout,
            operand: op,
            calls,
        },
    }
}

/// `dbg(op)`: a borrowed operand, whose helper calls a written `to_string` when `calls`.
fn debugging(op: Operand, calls: bool) -> Statement {
    Statement::Assign {
        dest: None,
        value: Rvalue::Dbg {
            at: Span::new(0, 0),
            text: "s".to_string(),
            operand: op,
            calls,
        },
    }
}

/// `print(op)`, which calls no written method.
fn show(op: Operand) -> Statement {
    printing(op, false)
}

fn borrow(binding: LocalId, owner: LocalId) -> Statement {
    assign(binding, Rvalue::Borrow(payload(owner)))
}

/// The commit of a binding: `b = copy b`.
fn commit(binding: LocalId) -> Statement {
    assign(binding, Rvalue::Copy(whole(binding)))
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

/// A release leaves the freed pointer in the slot, so a scope release after it would free it
/// again; a move leaves the slot null, which a scope release skips.
#[test]
fn scope_release_after_a_release() {
    for first in [Statement::Release, Statement::ScopeRelease] {
        let mut b = Builder::new("f");
        let s = b.local("s", Type::STRING);
        b.block(
            vec![literal(s), first(s), Statement::ScopeRelease(s)],
            ret(),
        );
        rejects(&b.finish(), 0, s, Rule::Consume);
    }
}

/// A move needs an owned local: one moved or released before is rejected.
#[test]
fn move_after_a_move_or_a_release() {
    for moved in [false, true] {
        let mut b = Builder::new("f");
        let s = b.local("s", Type::STRING);
        let t = b.local("t", Type::STRING);
        let u = b.local("u", Type::STRING);
        let mut body = vec![literal(s)];
        if moved {
            body.extend([
                assign(u, Rvalue::Use(Operand::Move(s))),
                Statement::Release(u),
            ]);
        } else {
            body.push(Statement::Release(s));
        }
        body.extend([
            assign(t, Rvalue::Use(Operand::Move(s))),
            Statement::Release(t),
        ]);
        b.block(body, ret());
        rejects(&b.finish(), 0, s, Rule::Consume);
    }
}

/// A scope release of a moved local frees nothing and leaves the slot null, so another may follow.
#[test]
fn scope_release_twice_after_a_move() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    let t = b.local("t", Type::STRING);
    b.block(
        vec![
            literal(s),
            assign(t, Rvalue::Use(Operand::Move(s))),
            Statement::Release(t),
            Statement::ScopeRelease(s),
            Statement::ScopeRelease(s),
        ],
        ret(),
    );
    assert_eq!(certify(&b.finish()), Ok(()));
}

/// `release? s` before an assignment, as lowering emits it, leaves `s` free to assign.
#[test]
fn assignment_after_a_scope_release() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    b.block(
        vec![
            literal(s),
            Statement::ScopeRelease(s),
            literal(s),
            Statement::ScopeRelease(s),
        ],
        ret(),
    );
    assert_eq!(certify(&b.finish()), Ok(()));
}

/// `s`, scoped at the loop body `bb2`, is released there and comes back round the loop to
/// `bb2`'s scope release still holding the freed pointer.
#[test]
fn released_local_comes_back_to_a_scope_release() {
    let mut b = Builder::new("f");
    let c = b.param("c", Type::BOOL);
    let s = b.local("s", Type::STRING);
    b.scope(s, BlockId(2));
    b.block(vec![], goto(1));
    b.block(vec![], branch(c, 2, 3));
    b.block(
        vec![
            Statement::ScopeRelease(s),
            literal(s),
            Statement::Release(s),
        ],
        goto(1),
    );
    b.block(vec![], ret());
    rejects(&b.finish(), 2, s, Rule::Consume);
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

/// A `dbg` reads its operand as `print` does, so a moved-out local is rejected.
#[test]
fn dbg_should_be_rejected_when_it_reads_a_moved_out_local() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    let t = b.local("t", Type::STRING);
    b.block(
        vec![
            literal(s),
            assign(t, Rvalue::Use(Operand::Move(s))),
            debugging(read(s), false),
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
        vec![call(
            vec![Operand::Read(first(a)), Operand::Move(a)],
            vec![false, false],
        )],
        ret(),
    );
    rejects(&b.finish(), 0, a, Rule::Borrow);
}

/// `f(s)` passes `s` without a copy or a move, so `f` and `s` would both own its reference.
#[test]
fn consuming_argument_read_in_place() {
    let mut b = Builder::new("f");
    let s = b.param("s", Type::STRING);
    b.block(
        vec![call(vec![read(s)], vec![false]), Statement::Release(s)],
        ret(),
    );
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

/// A `None` stored into `a[0]` counts whatever its depth, and for an optional of a counted enum
/// too: on a `[string??]` or a `[F?]`, for an `F` that holds a `string`, the store needs a `take`
/// of the old element, which then certifies once the taken value is released.
#[test]
fn nested_or_enum_none_stored_should_need_a_take() {
    let f = Type::intern(TypeKind::Enum(EnumId(0), TypeArgs::NONE));
    for element in [
        Type::optional(Type::optional(Type::STRING)),
        Type::optional(f),
    ] {
        let none = |a| Statement::Store {
            place: first(a),
            value: Operand::Const(Const::None {
                ty: element,
                counted: true,
            }),
        };
        let mut b = Builder::new("f");
        let a = b.param("a", Type::array(element));
        b.block(vec![none(a), Statement::Release(a)], ret());
        rejects(&b.finish(), 0, a, Rule::Take);
        let mut b = Builder::new("f");
        let a = b.param("a", Type::array(element));
        let old = b.local("old", element);
        b.function.locals[old.0].counted = true;
        b.block(
            vec![
                Statement::Take {
                    dest: old,
                    place: first(a),
                },
                none(a),
                Statement::Release(old),
                Statement::Release(a),
            ],
            ret(),
        );
        assert_eq!(certify(&b.finish()), Ok(()), "{element:?}");
    }
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

/// `break outer` from a `loop` in a guard ends the guard's bindings, which the outer loop's frame
/// holds no part of: the `loop`'s own frame starts inside the guard, so ending only what that
/// frame holds would leave `x` and `y` borrowed past the edge.
#[test]
fn labeled_break_from_guard_ends_its_borrows() {
    let program = certifies(
        "enum P {\n    Two(string, string)\n    Zero\n}\n\
         fn g(ps: [P], c: bool) -> i64 {\n    outer: for q in ps {\n        for p in ps {\n            match p {\n                Two(x, y) if (loop { if c { break outer }; break true }) => print(x)\n                _ => print(0)\n            }\n        }\n    }\n    0\n}",
    );
    let g = &program.functions[0];
    let (x, y) = (named(g, "x"), named(g, "y"));
    // The `break outer` edge ends both bindings and leaves both loops.
    assert!(g.blocks.iter().any(|block| {
        matches!(block.stmts.as_slice(),
            [Statement::End(first), Statement::End(second), ..] if (*first, *second) == (x, y))
            && matches!(block.term, Terminator::Goto(to) if !g.loops[0].blocks.contains(&to))
    }));
}

/// `break outer` in an argument releases the arguments already evaluated: the first `make()`.
#[test]
fn jump_out_of_half_built_call() {
    let program = certifies(
        "fn make() -> string { \"m\" }\n\
         fn g(a: string, b: string) {}\n\
         fn f(c: bool, xs: [i64]) {\n    outer: for x in xs {\n        for y in xs {\n            g(make(), { if c { break outer }; make() })\n        }\n    }\n}",
    );
    let f = &program.functions[2];
    assert_eq!(f.name, "f");
    // The block that holds the first `make()` branches on `c`; its `then` releases that result
    // and leaves both loops.
    let (first, then) = f
        .blocks
        .iter()
        .find_map(|block| match (block.stmts.last(), &block.term) {
            (
                Some(Statement::Assign {
                    dest: Some(dest),
                    value: Rvalue::Call { .. },
                }),
                Terminator::Branch { then, .. },
            ) => Some((*dest, *then)),
            _ => None,
        })
        .expect("the first argument's call should end the block that branches on `c`");
    let then = &f.blocks[then.0];
    assert!(
        then.stmts
            .iter()
            .any(|stmt| matches!(stmt, Statement::Release(l) if *l == first)),
        "{then:?}"
    );
    assert!(matches!(then.term, Terminator::Goto(to) if !f.loops[0].blocks.contains(&to)));
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
            call(vec![Operand::Move(s)], vec![false]),
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

/// `a[0] = None` on an array of optional strings, objects or arrays overwrites the element's
/// reference, so it needs a `take`.
#[test]
fn none_stored_with_no_take() {
    for payload in [
        Type::STRING,
        Type::intern(TypeKind::Class(ClassId(0), TypeArgs::NONE)),
        Type::array(Type::I64),
    ] {
        let mut b = Builder::new("f");
        let a = b.param("a", Type::array(Type::optional(payload)));
        b.block(
            vec![
                Statement::Store {
                    place: first(a),
                    value: Operand::Const(Const::None {
                        ty: Type::optional(payload),
                        counted: true,
                    }),
                },
                Statement::Release(a),
            ],
            ret(),
        );
        assert_eq!(
            certify(&b.finish()),
            Err(Violation {
                block: BlockId(0),
                local: a,
                rule: Rule::Take
            }),
            "{payload:?}"
        );
    }
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

/// The `return` arm of Task 2's program, with the binding's release, which elaboration made a
/// plain `release` since `c` is owned there, taken off its edge.
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
                    .any(|stmt| matches!(stmt, Statement::Release(local) if *local == c))
        })
        .expect("the `return` arm should release `c` on its way out");
    walk.blocks[exit]
        .stmts
        .retain(|stmt| !matches!(stmt, Statement::Release(local) if *local == c));
    rejects(walk, exit, c, Rule::Return);
}

/// Hand-built: the lowering releases a fresh value before the back edge, so no source program
/// leaks on it.
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

// Rule 3's other clauses: a write to a borrow's root ends it, and so does a clobber when it
// borrows through a field or an element.

/// `local.0`, a `mut` field.
fn field(local: LocalId) -> Place {
    Place {
        projections: vec![Projection::Field {
            index: 0,
            mutable: true,
        }],
        ..whole(local)
    }
}

/// `local.0`, a field that is not `mut`.
fn fixed(local: LocalId) -> Place {
    Place {
        projections: vec![Projection::Field {
            index: 0,
            mutable: false,
        }],
        ..whole(local)
    }
}

/// `place = 1`: a store of no reference, with no `take`.
fn store_int(place: Place) -> Statement {
    Statement::Store {
        place,
        value: Operand::Const(Const::Int(1, Type::I64)),
    }
}

fn object() -> Type {
    Type::intern(TypeKind::Class(ClassId(0), TypeArgs::NONE))
}

/// `b = borrow xs` in one block, then `make_unique xs` and a read of `b` in the next: the
/// write comes before `b`'s first mention in its block, while `b` is borrowed on entry.
#[test]
fn borrow_is_ended_by_a_write_before_its_first_mention_in_a_block() {
    let mut b = Builder::new("f");
    let xs = b.param("xs", Type::array(Type::I64));
    let x = b.local("b", Type::array(Type::I64));
    b.owner(x, xs);
    b.block(vec![assign(x, Rvalue::Borrow(whole(xs)))], goto(1));
    b.block(
        vec![
            Statement::MakeUnique(whole(xs)),
            show(read(x)),
            Statement::End(x),
            Statement::Release(xs),
        ],
        ret(),
    );
    rejects(&b.finish(), 1, x, Rule::Borrow);
}

/// `b = borrow xs`, then `make_unique xs` or `xs[0] = 1`, then a read of `b`: copy-on-write
/// may give `xs` a new buffer and release the one `b` reads.
#[test]
fn borrow_is_ended_by_a_write_to_its_root() {
    for store in [false, true] {
        let label = if store { "xs[0] = 1" } else { "make_unique xs" };
        let mut b = Builder::new("f");
        let xs = b.param("xs", Type::array(Type::I64));
        let x = b.local("b", Type::array(Type::I64));
        b.owner(x, xs);
        let write = if store {
            store_int(first(xs))
        } else {
            Statement::MakeUnique(whole(xs))
        };
        b.block(
            vec![
                assign(x, Rvalue::Borrow(whole(xs))),
                write,
                show(read(x)),
                Statement::End(x),
                Statement::Release(xs),
            ],
            ret(),
        );
        assert_eq!(
            certify(&b.finish()),
            Err(Violation {
                block: BlockId(0),
                local: x,
                rule: Rule::Borrow
            }),
            "{label}"
        );
    }
}

/// `b = borrow xs[0]`, then a call, which can replace the element.
#[test]
fn borrow_through_an_element_is_ended_by_a_call() {
    let mut b = Builder::new("f");
    let xs = b.param("xs", Type::array(Type::STRING));
    let x = b.local("b", Type::STRING);
    b.owner(x, xs);
    b.block(
        vec![
            assign(x, Rvalue::Borrow(first(xs))),
            call(vec![], vec![]),
            show(read(x)),
            Statement::End(x),
            Statement::Release(xs),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, x, Rule::Borrow);
}

/// `_t = take p.0`, `b = borrow o.0`, then `p.0 = move s` and `release _t`: when `p` is `o`,
/// the take moved `o.0`'s string into `_t`, and `release _t` frees it before `b` is read. The
/// take comes before the borrow, so only the store can end it.
#[test]
fn borrow_through_a_field_is_ended_by_a_counted_store() {
    let mut b = Builder::new("f");
    let o = b.param("o", object());
    let p = b.param("p", object());
    let x = b.local("b", Type::STRING);
    let value = b.local("s", Type::STRING);
    let old = b.local("_t", Type::STRING);
    b.owner(x, o);
    b.block(
        vec![
            fresh(value),
            Statement::Take {
                dest: old,
                place: field(p),
            },
            assign(x, Rvalue::Borrow(field(o))),
            Statement::Store {
                place: field(p),
                value: Operand::Move(value),
            },
            Statement::Release(old),
            show(read(x)),
            Statement::End(x),
            Statement::Release(o),
            Statement::Release(p),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, x, Rule::Borrow);
}

/// `b = borrow o.0`, then `make_unique p.0`. It frees nothing, but it decides on the count:
/// when `p` is `o` and `b` holds none, it finds the value unique and writes into it in place.
#[test]
fn borrow_through_a_field_is_ended_by_make_unique() {
    let mut b = Builder::new("f");
    let o = b.param("o", object());
    let p = b.param("p", object());
    let x = b.local("b", Type::array(Type::I64));
    b.owner(x, o);
    b.block(
        vec![
            assign(x, Rvalue::Borrow(field(o))),
            Statement::MakeUnique(field(p)),
            show(read(x)),
            Statement::End(x),
            Statement::Release(o),
            Statement::Release(p),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, x, Rule::Borrow);
}

/// `n` borrows of `xs` start and end, then `n` writes to `xs` follow, then the same locals
/// borrow `xs` again. No borrow is live across a write, and each region tests the writes with
/// one query per gap between its events, so the reads grow near linearly with `n`.
#[test]
fn certify_should_read_root_kills_across_ended_borrows_linearly() {
    let reads = |n: usize| {
        let mut b = Builder::new("f");
        let xs = b.param("xs", Type::array(Type::I64));
        let locals: Vec<_> = (0..n)
            .map(|i| {
                let x = b.local(&format!("b_{i}"), Type::array(Type::I64));
                b.owner(x, xs);
                x
            })
            .collect();
        let mut stmts = Vec::new();
        for &x in &locals {
            stmts.extend([assign(x, Rvalue::Borrow(whole(xs))), Statement::End(x)]);
        }
        stmts.extend((0..n).map(|_| Statement::MakeUnique(whole(xs))));
        for &x in &locals {
            stmts.extend([assign(x, Rvalue::Borrow(whole(xs))), Statement::End(x)]);
        }
        stmts.push(Statement::Release(xs));
        b.block(stmts, ret());
        READS.set(0);
        assert_eq!(certify(&b.finish()), Ok(()));
        READS.get()
    };
    assert_near_linear(reads(100), reads(200), reads(300));
}

/// `b = borrow xs`, `make_unique xs`, `show b`: the write to the root lies between two events
/// of the region, so the query for that gap finds it.
#[test]
fn certify_should_reject_a_root_write_between_two_events_of_a_borrow() {
    let mut b = Builder::new("f");
    let xs = b.param("xs", Type::array(Type::I64));
    let x = b.local("b", Type::array(Type::I64));
    b.owner(x, xs);
    b.block(
        vec![
            assign(x, Rvalue::Borrow(whole(xs))),
            Statement::MakeUnique(whole(xs)),
            show(read(x)),
            Statement::End(x),
            Statement::Release(xs),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, x, Rule::Borrow);
}

/// `b = borrow xs`, then `release xs` as the last statement: the write lies after the
/// region's last event, so the test after the loop finds it.
#[test]
fn certify_should_reject_a_root_write_after_the_last_event_of_a_borrow() {
    let mut b = Builder::new("f");
    let xs = b.param("xs", Type::array(Type::I64));
    let x = b.local("b", Type::array(Type::I64));
    b.owner(x, xs);
    b.block(
        vec![assign(x, Rvalue::Borrow(whole(xs))), Statement::Release(xs)],
        ret(),
    );
    rejects(&b.finish(), 0, x, Rule::Borrow);
}

/// `u = borrow t` with `t = borrow xs[0]`: `u` reads the element too, so a call after `t` ends
/// still ends `u`.
#[test]
fn chain_member_keeps_its_heads_clobbers() {
    let mut b = Builder::new("f");
    let xs = b.param("xs", Type::array(Type::STRING));
    let t = b.local("t", Type::STRING);
    let u = b.local("u", Type::STRING);
    b.owner(t, xs);
    b.owner(u, xs);
    b.block(
        vec![
            assign(t, Rvalue::Borrow(first(xs))),
            assign(u, Rvalue::Borrow(whole(t))),
            Statement::End(t),
            call(vec![], vec![]),
            show(read(u)),
            Statement::End(u),
            Statement::Release(xs),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, u, Rule::Borrow);
}

/// The same with `t = borrow o.0`, an enum, and `u` a borrow of its payload.
#[test]
fn payload_of_an_unstable_borrow_keeps_its_clobbers() {
    let mut b = Builder::new("f");
    let o = b.param("o", object());
    // `Builder::local` cannot count an enum without the enum table, so `t` is retyped below.
    let t = b.local("t", Type::STRING);
    let u = b.local("u", Type::STRING);
    b.owner(t, o);
    b.owner(u, o);
    b.block(
        vec![
            assign(t, Rvalue::Borrow(field(o))),
            borrow(u, t),
            Statement::End(t),
            call(vec![], vec![]),
            show(read(u)),
            Statement::End(u),
            Statement::Release(o),
        ],
        ret(),
    );
    let mut function = b.finish();
    function.locals[t.0].ty = Type::intern(TypeKind::Enum(EnumId(0), TypeArgs::NONE));
    rejects(&function, 0, u, Rule::Borrow);
}

/// `u = borrow t` in `bb1` comes before `t = borrow o.0` in `bb2` in block order, though `bb2`
/// runs first.
#[test]
fn instability_reaches_a_borrow_in_an_earlier_block() {
    let mut b = Builder::new("f");
    let o = b.param("o", object());
    let t = b.local("t", Type::STRING);
    let u = b.local("u", Type::STRING);
    b.owner(t, o);
    b.owner(u, o);
    b.block(vec![], goto(2));
    b.block(
        vec![
            assign(u, Rvalue::Borrow(whole(t))),
            Statement::End(t),
            call(vec![], vec![]),
            show(read(u)),
            Statement::End(u),
            Statement::Release(o),
        ],
        ret(),
    );
    b.block(vec![assign(t, Rvalue::Borrow(field(o)))], goto(1));
    rejects(&b.finish(), 1, u, Rule::Borrow);
}

/// `b = borrow o.0`, then `_t = take p.0` and `release _t`, which frees `o.0`'s string when `p`
/// is `o`. The store that puts a value back comes after `end b`, so only the take can end it.
#[test]
fn borrow_through_a_field_is_ended_by_a_counted_take() {
    let mut b = Builder::new("f");
    let o = b.param("o", object());
    let p = b.param("p", object());
    let x = b.local("b", Type::STRING);
    let value = b.local("s", Type::STRING);
    let old = b.local("_t", Type::STRING);
    b.owner(x, o);
    b.block(
        vec![
            fresh(value),
            assign(x, Rvalue::Borrow(field(o))),
            Statement::Take {
                dest: old,
                place: field(p),
            },
            Statement::Release(old),
            show(read(x)),
            Statement::End(x),
            Statement::Store {
                place: field(p),
                value: Operand::Move(value),
            },
            Statement::Release(o),
            Statement::Release(p),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, x, Rule::Borrow);
}

/// `b = borrow xs[0]`, then a call at the end of `bb0`, and the read in `bb1`: the gap after
/// `b`'s last event in `bb0` holds the call.
#[test]
fn clobber_after_the_last_event_of_a_block() {
    let mut b = Builder::new("f");
    let xs = b.param("xs", Type::array(Type::STRING));
    let x = b.local("b", Type::STRING);
    b.owner(x, xs);
    b.block(
        vec![assign(x, Rvalue::Borrow(first(xs))), call(vec![], vec![])],
        goto(1),
    );
    b.block(
        vec![show(read(x)), Statement::End(x), Statement::Release(xs)],
        ret(),
    );
    rejects(&b.finish(), 0, x, Rule::Borrow);
}

/// `b = borrow xs[0]` in `bb0`, a call in `bb1`, which does not mention `b`, and the read in
/// `bb2`: `bb0` covers `bb1`, so the call is found there.
#[test]
fn clobber_in_a_covered_block() {
    let mut b = Builder::new("f");
    let xs = b.param("xs", Type::array(Type::STRING));
    let x = b.local("b", Type::STRING);
    b.owner(x, xs);
    b.block(vec![assign(x, Rvalue::Borrow(first(xs)))], goto(1));
    b.block(vec![call(vec![], vec![])], goto(2));
    b.block(
        vec![show(read(x)), Statement::End(x), Statement::Release(xs)],
        ret(),
    );
    rejects(&b.finish(), 1, x, Rule::Borrow);
}

/// `b = borrow o.0` through a `mut` field, then an `Equal` and a read of `b`: an `Equal` that
/// may call a written `eq` (`calls`) can store through the field, as a call can; one that cannot
/// is no clobber.
#[test]
fn borrow_across_a_flagged_equal_should_be_rejected() {
    let case = |calls: bool| {
        let mut b = Builder::new("f");
        let o = b.param("o", object());
        let x = b.local("b", Type::STRING);
        let same = b.local("same", Type::BOOL);
        b.owner(x, o);
        let one = || Operand::Const(Const::Int(1, Type::I64));
        b.block(
            vec![
                assign(x, Rvalue::Borrow(field(o))),
                assign(
                    same,
                    Rvalue::Equal {
                        negated: false,
                        lhs: one(),
                        rhs: one(),
                        calls,
                        none: None,
                    },
                ),
                show(read(x)),
                Statement::End(x),
                Statement::Release(o),
            ],
            ret(),
        );
        (b.finish(), x)
    };
    let (flagged, x) = case(true);
    rejects(&flagged, 0, x, Rule::Borrow);
    let (plain, _) = case(false);
    assert_eq!(certify(&plain), Ok(()));
}

/// A `print` whose helper may call a written `to_string` (`calls`) can store through the field,
/// as a call can; one that cannot is no clobber.
#[test]
fn borrow_across_a_flagged_print_should_be_rejected() {
    let case = |calls: bool| {
        let mut b = Builder::new("f");
        let o = b.param("o", object());
        let x = b.local("b", Type::STRING);
        b.owner(x, o);
        b.block(
            vec![
                assign(x, Rvalue::Borrow(field(o))),
                printing(read(o), calls),
                show(read(x)),
                Statement::End(x),
                Statement::Release(o),
            ],
            ret(),
        );
        (b.finish(), x)
    };
    let (flagged, x) = case(true);
    rejects(&flagged, 0, x, Rule::Borrow);
    let (plain, _) = case(false);
    assert_eq!(certify(&plain), Ok(()));
}

/// A `dbg` whose helper may call a written `to_string` (`calls`) can store through the field, as
/// a flagged `print` can. A `dbg` that cannot is no clobber.
#[test]
fn borrow_across_a_flagged_dbg_should_be_rejected() {
    let case = |calls: bool| {
        let mut b = Builder::new("f");
        let o = b.param("o", object());
        let x = b.local("b", Type::STRING);
        b.owner(x, o);
        b.block(
            vec![
                assign(x, Rvalue::Borrow(field(o))),
                debugging(read(o), calls),
                show(read(x)),
                Statement::End(x),
                Statement::Release(o),
            ],
            ret(),
        );
        (b.finish(), x)
    };
    let (flagged, x) = case(true);
    rejects(&flagged, 0, x, Rule::Borrow);
    let (plain, _) = case(false);
    assert_eq!(certify(&plain), Ok(()));
}

/// `p.0 = 1` stores no reference, so it frees nothing a borrow of `o.0` reads.
#[test]
fn borrow_through_a_field_survives_an_uncounted_store() {
    let mut b = Builder::new("f");
    let o = b.param("o", object());
    let p = b.param("p", object());
    let x = b.local("b", Type::STRING);
    b.owner(x, o);
    b.block(
        vec![
            assign(x, Rvalue::Borrow(field(o))),
            store_int(field(p)),
            show(read(x)),
            Statement::End(x),
            Statement::Release(o),
            Statement::Release(p),
        ],
        ret(),
    );
    assert_eq!(certify(&b.finish()), Ok(()));
}

/// A `match` binding borrows a payload of its scrutinee, which no call can replace.
#[test]
fn borrow_through_a_payload_survives_a_call() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    let x = b.local("b", Type::STRING);
    b.owner(x, s);
    b.block(
        vec![
            fresh(s),
            borrow(x, s),
            call(vec![], vec![]),
            show(read(x)),
            Statement::End(x),
            Statement::Release(s),
        ],
        ret(),
    );
    assert_eq!(certify(&b.finish()), Ok(()));
}

/// A call cannot replace a whole local it is not handed.
#[test]
fn borrow_of_a_whole_local_survives_a_call() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    let x = b.local("b", Type::STRING);
    b.owner(x, s);
    b.block(
        vec![
            fresh(s),
            assign(x, Rvalue::Borrow(whole(s))),
            call(vec![], vec![]),
            show(read(x)),
            Statement::End(x),
            Statement::Release(s),
        ],
        ret(),
    );
    assert_eq!(certify(&b.finish()), Ok(()));
}

/// The counted-store case above, with the borrow through a field that is not `mut`. The borrow's
/// place decides stability, whatever field the store writes: no store can replace the field the
/// borrow reads, so the store does not end the borrow.
#[test]
fn borrow_through_a_fixed_field_survives_a_counted_store() {
    let mut b = Builder::new("f");
    let o = b.param("o", object());
    let p = b.param("p", object());
    let x = b.local("b", Type::STRING);
    let value = b.local("s", Type::STRING);
    let old = b.local("_t", Type::STRING);
    b.owner(x, o);
    b.block(
        vec![
            fresh(value),
            Statement::Take {
                dest: old,
                place: fixed(p),
            },
            assign(x, Rvalue::Borrow(fixed(o))),
            Statement::Store {
                place: fixed(p),
                value: Operand::Move(value),
            },
            Statement::Release(old),
            show(read(x)),
            Statement::End(x),
            Statement::Release(o),
            Statement::Release(p),
        ],
        ret(),
    );
    assert_eq!(certify(&b.finish()), Ok(()));
}

/// `b = borrow o.0` with `o.0` not `mut`: a call cannot replace the field.
#[test]
fn borrow_through_a_fixed_field_survives_a_call() {
    let mut b = Builder::new("f");
    let o = b.param("o", object());
    let x = b.local("b", Type::STRING);
    b.owner(x, o);
    b.block(
        vec![
            assign(x, Rvalue::Borrow(fixed(o))),
            call(vec![], vec![]),
            show(read(x)),
            Statement::End(x),
            Statement::Release(o),
        ],
        ret(),
    );
    assert_eq!(certify(&b.finish()), Ok(()));
}

/// `s = borrow o.0`, an optional field that is not `mut`, then `b = borrow (s as some)`: both
/// steps are stable, so a call ends neither borrow. `o` is a borrowed parameter, as `self` is in
/// `self.left?.check()`.
#[test]
fn borrow_through_a_some_survives_a_call() {
    let mut b = Builder::new("f");
    let o = b.param("o", object());
    b.borrowed(o);
    let s = b.local("s", Type::optional(object()));
    let x = b.local("b", object());
    b.owner(s, o);
    b.owner(x, s);
    b.block(
        vec![
            assign(s, Rvalue::Borrow(fixed(o))),
            assign(
                x,
                Rvalue::Borrow(Place {
                    projections: vec![Projection::Some],
                    ..whole(s)
                }),
            ),
            call(vec![], vec![]),
            show(read(x)),
            Statement::End(x),
            Statement::End(s),
        ],
        ret(),
    );
    assert_eq!(certify(&b.finish()), Ok(()));
}

// Borrowed parameters, and the arguments a call borrows.

/// `p` is borrowed from the caller for the whole body, so it may reach `return` borrowed.
#[test]
fn borrowed_parameter_reaches_return_borrowed() {
    let mut b = Builder::new("f");
    let p = b.param("p", Type::array(Type::I64));
    b.borrowed(p);
    b.block(vec![show(read(p))], ret());
    assert_eq!(certify(&b.finish()), Ok(()));
}

#[test]
fn scope_release_of_a_borrowed_parameter_is_rejected() {
    let mut b = Builder::new("f");
    let p = b.param("p", Type::array(Type::I64));
    b.borrowed(p);
    b.block(vec![Statement::ScopeRelease(p)], ret());
    rejects(&b.finish(), 0, p, Rule::Consume);
}

#[test]
fn move_of_a_borrowed_parameter_is_rejected() {
    let mut b = Builder::new("f");
    let p = b.param("p", Type::array(Type::I64));
    b.borrowed(p);
    let t = b.local("t", Type::array(Type::I64));
    b.block(
        vec![
            assign(t, Rvalue::Use(Operand::Move(p))),
            Statement::Release(t),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, p, Rule::Consume);
}

/// A move into a borrowed position hands nothing on: `t` is still owned at `return`.
#[test]
fn move_into_a_borrowed_position_leaks() {
    let mut b = Builder::new("f");
    let t = b.local("t", Type::array(Type::I64));
    b.block(
        vec![fresh(t), call(vec![Operand::Move(t)], vec![true])],
        ret(),
    );
    rejects(&b.finish(), 0, t, Rule::Return);
}

#[test]
fn read_at_a_borrowed_position_is_accepted() {
    let mut b = Builder::new("f");
    let t = b.local("t", Type::array(Type::I64));
    b.block(
        vec![
            fresh(t),
            call(vec![read(t)], vec![true]),
            Statement::Release(t),
        ],
        ret(),
    );
    assert_eq!(certify(&b.finish()), Ok(()));
}

/// An owned position consumes its argument, and a read in place hands on a reference `t` keeps.
#[test]
fn read_at_an_owned_position_is_rejected() {
    let mut b = Builder::new("f");
    let t = b.local("t", Type::array(Type::I64));
    b.block(vec![fresh(t), call(vec![read(t)], vec![false])], ret());
    rejects(&b.finish(), 0, t, Rule::Consume);
}

/// On mir as lowering left it, the visitor reports a `use` and a guard mark as the move each may
/// become, where the certifier's own walk panics.
#[test]
fn lowered_use_and_guard_consume_their_local() {
    for op in [Operand::Use(LocalId(0)), Operand::Guarded(LocalId(0))] {
        let stmt = assign(LocalId(1), Rvalue::Use(op));
        let mut effects = Vec::new();
        statement(&stmt, true, &mut |local, effect, _| {
            effects.push((local, effect));
        });
        assert!(effects == [(LocalId(0), Effect::Consume), (LocalId(1), Effect::Assign)]);
    }
}

/// The certifier reads mir after the ownership pass, where no `use` is left: one there is a bug.
#[test]
#[should_panic(expected = "the ownership pass should leave no `use` and no guard mark")]
fn use_after_the_ownership_pass_panics() {
    let mut b = Builder::new("f");
    let s = b.param("s", Type::STRING);
    let t = b.local("t", Type::STRING);
    b.block(
        vec![
            assign(t, Rvalue::Use(Operand::Use(s))),
            Statement::Release(t),
        ],
        ret(),
    );
    let _ = certify(&b.finish());
}

/// `make_unique b` with `b` a borrow of the array `xs` finds the buffer unique, though `xs`
/// holds it too, and so writes into `xs` in place.
#[test]
fn write_into_a_borrowed_array_is_rejected() {
    let mut b = Builder::new("f");
    let xs = b.param("xs", Type::array(Type::I64));
    let x = b.local("b", Type::array(Type::I64));
    b.owner(x, xs);
    b.block(
        vec![
            assign(x, Rvalue::Borrow(whole(xs))),
            Statement::MakeUnique(whole(x)),
            Statement::End(x),
            Statement::Release(xs),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, x, Rule::WriteBorrowed);
}

/// `b.0 = 1` with `b` a borrow of the object `o` writes the object both name, as `o.0 = 1`
/// would.
#[test]
fn store_through_a_borrowed_object_is_accepted() {
    let mut b = Builder::new("f");
    let o = b.param("o", object());
    let x = b.local("b", object());
    b.owner(x, o);
    b.block(
        vec![
            assign(x, Rvalue::Borrow(whole(o))),
            store_int(field(x)),
            Statement::End(x),
            Statement::Release(o),
        ],
        ret(),
    );
    assert_eq!(certify(&b.finish()), Ok(()));
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
    let o = b.param(
        "o",
        Type::intern(TypeKind::Class(ClassId(0), TypeArgs::NONE)),
    );
    let t = b.local("t", Type::STRING);
    let u = b.local("u", Type::STRING);
    let old = b.local("old", Type::STRING);
    let field = |index| Place {
        projections: vec![Projection::Field {
            index,
            mutable: true,
        }],
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

/// A local lowering never reached, scoped in dead code after a `return`, is never checked: its
/// scope block is in no region.
#[test]
fn unmentioned_local_is_ignored() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    b.scope(s, BlockId(1));
    b.block(vec![], ret());
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

/// Sol's case: `x`, scoped at `bb1`, leaves it ended into the loop `bb2`, `bb4`. The back edge
/// `bb4` to `bb2` meets the tree at `bb1`'s depth, but it re-enters only `bb2`, a block below
/// `bb1`, so no ended `x` comes back to `bb1`'s scope release.
#[test]
fn edge_to_a_block_below_the_scope_block_does_not_come_back() {
    let mut b = Builder::new("f");
    let c = b.param("c", Type::BOOL);
    let s = b.param("s", Type::STRING);
    let x = b.local("x", Type::STRING);
    b.scope(x, BlockId(1));
    b.owner(x, s);
    b.block(vec![], branch(c, 1, 5));
    b.block(
        vec![Statement::ScopeRelease(x), borrow(x, s), Statement::End(x)],
        branch(c, 2, 3),
    );
    b.block(vec![], goto(4));
    b.block(vec![], goto(4));
    b.block(vec![], branch(c, 2, 5));
    b.block(vec![Statement::Release(s)], ret());
    assert_eq!(certify(&b.finish()), Ok(()));
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

/// A presence test reads `o` in place and moves nothing, so `o` is still owned on both edges:
/// a read and a release there certify, and a `return` without the release leaks.
#[test]
fn test_present_should_leave_its_place_owned_on_both_edges() {
    for leak in [None, Some(1), Some(2)] {
        let mut b = Builder::new("f");
        let o = b.local("o", Type::optional(Type::STRING));
        b.block(
            vec![fresh(o)],
            Terminator::TestPresent {
                place: whole(o),
                some: BlockId(1),
                none: BlockId(2),
            },
        );
        for edge in [1, 2] {
            let mut stmts = vec![show(read(o))];
            if leak != Some(edge) {
                stmts.push(Statement::Release(o));
            }
            b.block(stmts, ret());
        }
        let f = b.finish();
        match leak {
            None => assert_eq!(certify(&f), Ok(())),
            Some(edge) => rejects(&f, edge, o, Rule::Return),
        }
    }
}

/// A binding under `Some` in the middle of a path borrows as one under a payload does, from an
/// owned scrutinee and from a borrowed one.
#[test]
fn a_borrow_through_a_mid_path_some_should_certify() {
    let program = certifies(
        "enum Shape {\n    Circle(string)\n    Square(i64)\n}\nfn f(s: Shape?) -> i64 {\n    match s {\n        Circle(r) => r.len()\n        Square(n) => n\n        None => 0\n    }\n}\nfn g() -> i64 {\n    let s: Shape? = Shape.Circle(\"ab\")\n    match s {\n        Circle(r) => r.len()\n        _ => 0\n    }\n}",
    );
    for name in ["f", "g"] {
        let f = program
            .functions
            .iter()
            .find(|f| f.name == name)
            .expect("the program should have the function");
        let r = named(f, "r");
        let mid_path = f.blocks.iter().flat_map(|block| &block.stmts).any(|stmt| {
            matches!(stmt, Statement::Assign { dest: Some(dest), value: Rvalue::Borrow(place) }
                if *dest == r && place.projections.first() == Some(&Projection::Some)
                    && place.projections.len() == 2)
        });
        assert!(mid_path, "fn {name} should borrow `r` through `Some`");
    }
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

/// `certified`, the check `build` runs in a debug build, panics on the first violation with a
/// message naming the function, the block, the local and the rule.
#[test]
#[should_panic(expected = "mir certifier: fn f, bb0, s#0: may still own a reference at `return`")]
fn violation_message_names_where_and_what() {
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
/// Each `s_i`'s region is its own block and that last one, so the reads grow near linearly
/// with the locals.
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
    assert_near_linear(edges(n), edges(2 * n), edges(3 * n));
}

/// Each `s_i` is moved on one arm of its `if`, so it keeps a conditional release at the
/// function's end. Only the locals a scope release names are run, so the reads grow near
/// linearly with them.
#[test]
fn scope_release_states_are_sparse() {
    let source = |n: usize| {
        let body = (0..n)
            .map(|i| format!("    let s_{i} = \"x\"\n    if c {{ keep(s_{i}) }}\n"))
            .collect::<Vec<_>>()
            .concat();
        format!("fn keep(s: string) {{}}\nfn f(c: bool) {{\n{body}}}")
    };
    let reads = |n: usize| {
        let program = build(&checked(&source(n)));
        let f = program
            .functions
            .iter()
            .find(|f| f.name == "f")
            .expect("the program should have `f`");
        READS.set(0);
        assert_eq!(scope_release_states(f, &flagged(f)).len(), n);
        READS.get()
    };
    let n = 100;
    assert_near_linear(reads(n), reads(2 * n), reads(3 * n));
}

/// `while D_n {} print(s)`, with `D_0 = c` and `D_{n+1} = { while D_n {} c }`: block `k` of
/// the nesting has every loop head around it in its dominance frontier, so storing every
/// block's frontier grows as `n^2`. The reads grow near linearly with the levels.
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
    assert_near_linear(reads(10), reads(20), reads(30));
}

/// A chain of `n` blocks that each read the parameter `s`, then a switch back to every one of
/// them: each chain block is a join with an edge from the switch, so a frontier search that
/// searched a subtree twice would find those edges again, `n` of them each time. The exit is
/// also reached from `bb0`, so the switch is the last block of every chain block's subtree.
#[test]
fn switch_back_to_every_block_reads_linearly() {
    let reads = |n: usize| {
        let mut b = Builder::new("f");
        let c = b.param("c", Type::BOOL);
        let key = b.param("key", Type::I64);
        let s = b.param("s", Type::STRING);
        b.block(vec![show(read(s))], branch(c, 1, n + 1));
        for i in 1..n {
            b.block(vec![show(read(s))], goto(i + 1));
        }
        let arms = (0..n)
            .map(|i| {
                let value = i64::try_from(i).expect("a block number should fit an i64");
                (Const::Int(value, Type::I64), BlockId(i))
            })
            .collect();
        b.block(
            vec![],
            Terminator::SwitchValue {
                operand: read(key),
                arms,
                otherwise: Some(BlockId(n + 1)),
            },
        );
        b.block(vec![Statement::Release(s)], ret());
        let f = b.finish();
        READS.set(0);
        assert_eq!(certify(&f), Ok(()));
        READS.get()
    };
    assert_near_linear(reads(100), reads(200), reads(300));
}

/// `n` calls, then `n` short borrows of `xs[0]`, all in one block: a walk over the block's
/// clobbers for each borrow is quadratic. The reads grow near linearly with `n`.
#[test]
fn clobbers_before_many_borrows_read_linearly() {
    let reads = |n: usize| {
        let mut b = Builder::new("f");
        let xs = b.param("xs", Type::array(Type::STRING));
        let mut stmts: Vec<Statement> = (0..n).map(|_| call(vec![], vec![])).collect();
        for i in 0..n {
            let x = b.local(&format!("b_{i}"), Type::STRING);
            b.owner(x, xs);
            stmts.extend([
                assign(x, Rvalue::Borrow(first(xs))),
                show(read(x)),
                Statement::End(x),
            ]);
        }
        stmts.push(Statement::Release(xs));
        b.block(stmts, ret());
        let f = b.finish();
        READS.set(0);
        assert_eq!(certify(&f), Ok(()));
        READS.get()
    };
    assert_near_linear(reads(100), reads(200), reads(300));
}

/// `n` writes to `xs`, then `n` short borrows of `xs`, all in one block, as pair removal makes
/// of `xs[0] = i` lines and then `print(xs == xs)` lines: a walk over the root's kills from
/// the block's start for each borrow is quadratic. The reads grow near linearly with `n`.
#[test]
fn root_writes_before_many_borrows_read_linearly() {
    let reads = |n: usize| {
        let mut b = Builder::new("f");
        let xs = b.param("xs", Type::array(Type::STRING));
        let mut stmts: Vec<Statement> = (0..n).map(|_| Statement::MakeUnique(whole(xs))).collect();
        for i in 0..n {
            let x = b.local(&format!("b_{i}"), Type::array(Type::STRING));
            b.owner(x, xs);
            stmts.extend([
                assign(x, Rvalue::Borrow(whole(xs))),
                show(read(x)),
                Statement::End(x),
            ]);
        }
        stmts.push(Statement::Release(xs));
        b.block(stmts, ret());
        let f = b.finish();
        READS.set(0);
        assert_eq!(certify(&f), Ok(()));
        READS.get()
    };
    assert_near_linear(reads(100), reads(200), reads(300));
}

/// `t = borrow o.0; u = borrow t; end u; end t`, `n` times in one block: `t` is a seed `n`
/// times over, and `t`'s `n` edges are walked once, not once per seed. The reads grow near
/// linearly with `n`.
#[test]
fn repeated_borrows_into_one_local_read_linearly() {
    let reads = |n: usize| {
        let mut b = Builder::new("f");
        let o = b.param("o", object());
        let t = b.local("t", Type::STRING);
        let u = b.local("u", Type::STRING);
        b.owner(t, o);
        b.owner(u, o);
        let mut stmts = Vec::new();
        for _ in 0..n {
            stmts.extend([
                assign(t, Rvalue::Borrow(field(o))),
                assign(u, Rvalue::Borrow(whole(t))),
                Statement::End(u),
                Statement::End(t),
            ]);
        }
        stmts.push(Statement::Release(o));
        b.block(stmts, ret());
        let function = b.finish();
        READS.set(0);
        assert_eq!(certify(&function), Ok(()));
        READS.get()
    };
    assert_near_linear(reads(100), reads(200), reads(300));
}

// The ported timeout tests, now through lowering, the ownership pass and the certifier, sized
// to check correctness; counted tests pin their cost.

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

/// The counted tests `wide_guard_probes_once_per_read` (lowering) and
/// `liveness_reads_grow_linearly` (ownership) pin the cost of these passes.
#[test]
fn many_guarded_arms_mark_last_uses() {
    const VARIANTS: usize = 100;
    let src = wide_match(VARIANTS, ", keep: bool", "", |_| " if keep".to_owned());
    // Each arm's `s` is read once, in its body, so every read is its last use.
    assert_eq!(moves_of_s(&src), VARIANTS);
}

/// The counted tests `liveness_reads_grow_linearly` (ownership) and
/// `scope_release_gap_is_sparse` (certifier) pin the cost of these passes.
#[test]
fn many_guards_of_distinct_locals_mark_last_uses() {
    const VARIANTS: usize = 100;
    let locals = (0..VARIANTS)
        .map(|i| format!("    let g{i} = c\n"))
        .collect::<Vec<_>>()
        .concat();
    let src = wide_match(VARIANTS, ", c: bool", &locals, |i| format!(" if g{i}"));
    assert_eq!(moves_of_s(&src), VARIANTS);
}

/// A guard walked again inside each guard around it is exponential in the depth, so this test
/// hangs instead of failing; its keep-alive override is in `.config/nextest.toml`.
#[test]
fn nested_guards_mark_last_uses() {
    const DEPTH: usize = 48;
    let guard = (0..DEPTH).fold("c".to_owned(), |inner, _| {
        format!("match c {{\n true if {inner} => true\n _ => false\n }}")
    });
    let src = format!(
        "enum E {{\n    V(string)\n    W\n}}\nfn f(e: E, c: bool) -> string {{\n    match e {{\n        V(s) if {guard} => s\n        _ => \"\"\n    }}\n}}"
    );
    assert_eq!(moves_of_s(&src), 1);
}

/// `match e` over `variants` variants, each arm `V{i}(s) => s`. Arm `i`'s test hangs off arm
/// `i - 1`'s failure edge, and the join has one predecessor per arm.
fn many_arms(variants: usize) -> String {
    let variant_list = (0..variants)
        .map(|i| format!("    V{i}(string)\n"))
        .collect::<Vec<_>>()
        .concat();
    let arms = (0..variants)
        .map(|i| format!("        V{i}(s) => s\n"))
        .collect::<Vec<_>>()
        .concat();
    format!(
        "enum E {{\n{variant_list}}}\nfn f(e: E) -> string {{\n    match e {{\n{arms}    }}\n}}"
    )
}

/// `arm_chain_reads_linearly` pins the cost of this shape.
#[test]
fn many_arms_mark_last_uses() {
    const VARIANTS: usize = 100;
    // Each arm's `s` is read once, so every read is its last use.
    assert_eq!(moves_of_s(&many_arms(VARIANTS)), VARIANTS);
}

/// The dominators of an arm chain: iterative dominators, or a dominator search without path
/// compression, are quadratic in the arms. The reads grow near linearly with them.
#[test]
fn arm_chain_reads_linearly() {
    let reads = |n: usize| {
        let program = build(&checked(&many_arms(n)));
        let f = &program.functions[0];
        READS.set(0);
        assert_eq!(certify(f), Ok(()));
        READS.get()
    };
    assert_near_linear(reads(100), reads(200), reads(300));
}

// Reuse tokens and `take_apart`.

/// `e`, an owned parameter of enum 0. `Builder::local` never counts an enum, so it is counted
/// here, as an enum with a box is.
pub(super) fn enum_param(b: &mut Builder) -> LocalId {
    let e = b.param("e", Type::intern(TypeKind::Enum(EnumId(0), TypeArgs::NONE)));
    b.function.locals[e.0].counted = true;
    e
}

/// A token of enum 0.
pub(super) fn token(b: &mut Builder) -> LocalId {
    b.token(
        "tok",
        Type::intern(TypeKind::Enum(EnumId(0), TypeArgs::NONE)),
    )
}

/// `take_apart scrutinee: tok = token (scrutinee as 0).0`, and each of `bindings` moved from
/// the same place.
pub(super) fn take_apart(scrutinee: LocalId, tok: LocalId, bindings: &[LocalId]) -> Statement {
    Statement::TakeApart {
        scrutinee,
        tokens: vec![(tok, payload(scrutinee))],
        bindings: bindings.iter().map(|&b| (b, payload(scrutinee))).collect(),
        rest: Vec::new(),
    }
}

/// Each iteration builds `x` and takes it apart, and the `continue` edge scope releases the
/// token, which would release the box's contents again.
#[test]
fn token_scope_released_on_a_continue_edge_is_rejected() {
    let mut b = Builder::new("f");
    let c = b.param("c", Type::BOOL);
    let x = b.local("x", Type::intern(TypeKind::Enum(EnumId(0), TypeArgs::NONE)));
    b.function.locals[x.0].counted = true;
    let tok = token(&mut b);
    b.scope(x, BlockId(1));
    b.scope(tok, BlockId(1));
    b.block(vec![], goto(1));
    b.block(
        vec![
            assign(
                x,
                Rvalue::Variant {
                    id: EnumId(0),
                    variant: 0,
                    args: Vec::new(),
                    reuse: Vec::new(),
                },
            ),
            take_apart(x, tok, &[]),
        ],
        branch(c, 2, 3),
    );
    b.block(vec![Statement::ScopeRelease(tok)], goto(1));
    b.block(vec![Statement::Free(tok)], ret());
    b.add_loop(BlockId(0), BlockId(1), vec![BlockId(1), BlockId(2)]);
    rejects(&b.finish(), 2, tok, Rule::Token);
}

#[test]
fn free_of_a_freed_token_is_rejected() {
    let mut b = Builder::new("f");
    let e = enum_param(&mut b);
    let tok = token(&mut b);
    b.block(
        vec![
            take_apart(e, tok, &[]),
            Statement::Free(tok),
            Statement::Free(tok),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, tok, Rule::Token);
}

/// A free of an empty token leaves it empty, as a scope release does, so a second free on the
/// same path frees `null` again.
#[test]
fn free_of_an_empty_token_leaves_it_empty() {
    let mut b = Builder::new("f");
    let tok = token(&mut b);
    b.block(vec![Statement::Free(tok), Statement::Free(tok)], ret());
    assert_eq!(certify(&b.finish()), Ok(()));
}

#[test]
fn token_leaked_on_one_path_is_rejected() {
    let mut b = Builder::new("f");
    let e = enum_param(&mut b);
    let c = b.param("c", Type::BOOL);
    let tok = token(&mut b);
    b.block(vec![take_apart(e, tok, &[])], branch(c, 1, 2));
    b.block(vec![Statement::Free(tok)], ret());
    b.block(vec![], ret());
    rejects(&b.finish(), 2, tok, Rule::Return);
}

/// The token is owned where the `match` took `e` apart and empty where it released `e`: the
/// `free` sees {owned, empty}, which its helper's `null` test covers.
#[test]
fn free_of_a_token_that_may_be_empty_is_accepted() {
    let mut b = Builder::new("f");
    let e = enum_param(&mut b);
    let c = b.param("c", Type::BOOL);
    let tok = token(&mut b);
    b.block(vec![], branch(c, 1, 2));
    b.block(vec![take_apart(e, tok, &[])], goto(3));
    b.block(vec![Statement::Release(e)], goto(3));
    b.block(vec![Statement::Free(tok)], ret());
    let f = b.finish();
    assert_eq!(certify(&f), Ok(()));
    let named: Vec<bool> = (0..f.locals.len()).map(|i| i == tok.0).collect();
    assert_eq!(
        scope_release_states(&f, &named).get(&(BlockId(3), 0)),
        Some(&(OWNED | EMPTY))
    );
}

#[test]
fn free_of_a_local_that_is_not_a_token_is_rejected() {
    let mut b = Builder::new("f");
    let s = b.local("s", Type::STRING);
    b.block(vec![literal(s), Statement::Free(s)], ret());
    rejects(&b.finish(), 0, s, Rule::Token);
}

/// `take_apart` consumes `e`, so a binding still borrowing from it breaks rule 3: lowering ends
/// each binding's borrow first.
#[test]
fn take_apart_while_a_binding_borrows_is_rejected() {
    let function = |end_first: bool| {
        let mut b = Builder::new("f");
        let e = enum_param(&mut b);
        let x = b.local("x", Type::STRING);
        let tok = token(&mut b);
        b.owner(x, e);
        let mut stmts = vec![borrow(x, e)];
        if end_first {
            stmts.push(Statement::End(x));
        }
        stmts.extend([
            take_apart(e, tok, &[x]),
            Statement::Release(x),
            Statement::Free(tok),
        ]);
        b.block(stmts, ret());
        (b.finish(), x)
    };
    let (f, x) = function(false);
    rejects(&f, 0, x, Rule::Borrow);
    assert_eq!(certify(&function(true).0), Ok(()));
}

/// `dest = variant V(_) into [tok]`, a construction of enum 0 that builds into `tok`.
pub(super) fn reused(dest: LocalId, tok: LocalId) -> Statement {
    assign(
        dest,
        Rvalue::Variant {
            id: EnumId(0),
            variant: 0,
            args: vec![Operand::Const(Const::Int(1, Type::I64))],
            reuse: vec![Some(tok)],
        },
    )
}

/// `take_apart e: tok = ..`, then `x = variant V(1) into [tok]`, then `free tok`.
pub(super) fn free_after_reuse() -> (Function, LocalId) {
    let mut b = Builder::new("f");
    let e = enum_param(&mut b);
    let x = b.local("x", Type::intern(TypeKind::Enum(EnumId(0), TypeArgs::NONE)));
    let tok = token(&mut b);
    b.block(
        vec![
            take_apart(e, tok, &[]),
            reused(x, tok),
            Statement::Free(tok),
        ],
        ret(),
    );
    (b.finish(), tok)
}

/// The construction empties the token, so the `free` sees {empty}.
#[test]
fn free_after_reuse_is_accepted() {
    let (f, tok) = free_after_reuse();
    assert_eq!(certify(&f), Ok(()));
    let named: Vec<bool> = (0..f.locals.len()).map(|i| i == tok.0).collect();
    assert_eq!(
        scope_release_states(&f, &named).get(&(BlockId(0), 2)),
        Some(&EMPTY)
    );
}

#[test]
fn reuse_of_a_token_twice_is_rejected() {
    let mut b = Builder::new("f");
    let e = enum_param(&mut b);
    let x = b.local("x", Type::intern(TypeKind::Enum(EnumId(0), TypeArgs::NONE)));
    let y = b.local("y", Type::intern(TypeKind::Enum(EnumId(0), TypeArgs::NONE)));
    let tok = token(&mut b);
    b.block(
        vec![
            take_apart(e, tok, &[]),
            reused(x, tok),
            reused(y, tok),
            Statement::Free(tok),
        ],
        ret(),
    );
    rejects(&b.finish(), 0, tok, Rule::Consume);
}
