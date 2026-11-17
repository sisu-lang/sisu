//! The tests of the inference table and the join that is built on it.

use super::*;
use crate::check::infer::{Kind, Written};

/// A result that holds no type, so that `with_table` has nothing to write back in the test.
struct Nothing<R>(R);

impl<R> WriteBack for Nothing<R> {
    fn write_back_all(&mut self, _table: &mut infer::Table) -> Written {
        Written::default()
    }
}

/// Runs `f` on a checker over `src`, whose names and interface signatures are collected, inside
/// `with_table`: the checker has the table of a function with no variable.
pub(in crate::check) fn with_checker<R>(src: &str, f: impl FnOnce(&mut Checker<'_>) -> R) -> R {
    let program = parse_ok(src);
    let mut checker = Checker::new(&program, src, "");
    let items: Vec<&ast::Item> = program.items.iter().collect();
    checker.collect_names(&items, items.len(), &program.broken);
    checker.interface_signatures(&items);
    checker.with_table(|c| Nothing(f(c))).0
}

/// Runs `f` inside `with_table`, in the body of `main` of `src` with `fn probe(<params>)` and
/// `fn main()` added, on the types of the parameters of `probe`. The prelude is declared too.
pub(in crate::check) fn with_params<R>(
    src: &str,
    params: &str,
    f: impl FnOnce(&mut Checker<'_>, &[Type]) -> R,
) -> R {
    let src = format!("{src}fn probe({params}) {{}}\nfn main() {{}}\n");
    let program = parse_ok(&src);
    let prelude = prelude_program("");
    let mut checker = Checker::new(&program, &src, &prelude.text);
    let items: Vec<&ast::Item> = program.items.iter().chain(&prelude.program.items).collect();
    checker.declare_items(&items, program.items.len(), &program.broken);
    let at = |name: &str| {
        (checker.signatures.iter())
            .position(|signature| signature.name == name)
            .expect("the program should declare the function")
    };
    let (probe, main) = (at("probe"), at("main"));
    let types: Vec<Type> = (checker.signatures[probe].params.iter())
        .map(|ty| ty.expect("each parameter of `probe` should name a type"))
        .collect();
    let body = Body {
        owner: Some(Owner::Func(tir::FuncId(main))),
        ..Body::default()
    };
    checker
        .with_table(|c| Nothing(c.with_body(body, |c| f(c, &types)).0))
        .0
}

#[test]
fn cache_should_not_keep_an_inference_variable_across_functions() {
    // GIVEN two functions whose first open `Any` variable has the same id and a different type
    let src = "interface Holds<T> {\n    fn get(self) -> T\n}\n";
    let program = parse_ok(src);
    let mut checker = Checker::new(&program, src, "");
    let items: Vec<&ast::Item> = program.items.iter().collect();
    checker.collect_names(&items, items.len(), &program.broken);
    checker.interface_signatures(&items);
    let Some(Declared::Known(holds)) = checker.names.interface(Symbol::intern("Holds")) else {
        panic!("Holds should be an interface");
    };
    let any_of = |arg| Type::intern(TypeKind::Any(holds, TypeArgs::intern(&[arg])));
    let mut closures = Vec::new();
    for settled in [Type::I64, Type::STRING] {
        // WHEN each function asks for the interfaces of `any Holds<A>`
        let closure = checker
            .with_table(|c| {
                let var = c.table.fresh(Kind::Any, Span::new(0, 1));
                assert_eq!(var, Type::intern(TypeKind::Infer(crate::types::InferId(0))));
                assert!(c.unify(var, settled), "A should unify with {settled:?}");
                let ty = any_of(var);
                let closure = c.any_interfaces(ty);
                // THEN the key that holds the variable is not kept, so the next lookup is computed
                assert!(!c.any_closures.contains_key(&ty));
                Nothing(closure.to_vec())
            })
            .0;
        closures.push(closure);
    }
    // A key with no variable is kept, as before.
    checker.any_interfaces(any_of(Type::I64));
    assert!(checker.any_closures.contains_key(&any_of(Type::I64)));
    assert!(closures.iter().all(|closure| !closure.is_empty()));
}

#[test]
fn cache_should_not_keep_a_conformance_leaf_or_probe_of_a_type_that_holds_a_variable() {
    // GIVEN a generic class and an interface with an associated type
    // The first interface stands for each language interface, which `conforms` answers at once.
    let src = "interface Pad {\n    fn pad(self) -> bool\n}\ninterface Source {\n    type Item\n    fn next(self) -> Item\n}\nclass Box<T> {\n    item: T\n}\n";
    let program = parse_ok(src);
    let mut checker = Checker::new(&program, src, "");
    let items: Vec<&ast::Item> = program.items.iter().collect();
    checker.collect_names(&items, items.len(), &program.broken);
    checker.interface_signatures(&items);
    let Some(Declared::Known(source)) = checker.names.interface(Symbol::intern("Source")) else {
        panic!("Source should be an interface");
    };
    let Some(Declared::Known(boxed)) = checker.names.class(Symbol::intern("Box")) else {
        panic!("Box should be a class");
    };
    // The marks that the pass before the bodies makes: `Box` does not grow.
    checker.growing_classes = vec![false];
    checker.with_table(|c| {
        let var = c.table.fresh(Kind::Any, Span::new(0, 1));
        let held = Type::intern(TypeKind::Class(boxed, TypeArgs::intern(&[var])));
        // WHEN the checker asks about `Box<A>`: a conformance, its failing part and its probe
        c.conforms(held, source, TypeArgs::NONE);
        c.failing_leaf(held, source, TypeArgs::NONE);
        c.probe_associated(held, source, TypeArgs::NONE);
        let closed = Type::intern(TypeKind::Class(boxed, TypeArgs::intern(&[Type::I64])));
        c.conforms(closed, source, TypeArgs::NONE);
        c.failing_leaf(closed, source, TypeArgs::NONE);
        // The key with no variable is kept, as before.
        assert!(c.conformed.contains_key(&(closed, source, TypeArgs::NONE)));
        // THEN no check-wide cache keeps a key that holds the variable
        assert!(
            !c.conformed
                .keys()
                .any(|(ty, _, args)| ty.holds_infer() || args.holds_infer())
        );
        assert!(
            !c.leaves
                .keys()
                .any(|(ty, _, args)| ty.holds_infer() || args.holds_infer())
        );
        assert!(
            !c.probed_types
                .iter()
                .any(|(ty, _, args)| ty.holds_infer() || args.holds_infer())
        );
        assert!(!c.probed_assoc.iter().any(|ty| ty.holds_infer()));
        Nothing(())
    });
}

/// The helpers that the join tests name. A row's body goes in `main`, after `LOCALS`.
const HELPERS: &str = "\
fn id<T>(x: T) -> T { x }\n\
fn pick<T>(a: T, b: T) -> T { a }\n\
fn first<T>(xs: [T]) -> T? { if xs.len() > 0 { Some(xs[0]) } else { None } }\n\
fn last<T>(xs: [T]) -> T { xs[xs.len() - 1] }\n\
fn big<T: Ord>(a: T, b: T) -> T { a }\n\
fn bump<T>(mut x: T, y: T) -> T { y }\n\
fn need<T: Seen>(x: T) -> T { x }\n\
fn g<T>(x: T?, y: T) -> T { y }\n\
fn f<T>(x: [T], y: T) -> T { y }\n\
fn h<T>(a: T, b: T?) -> T { a }\n\
fn bad(x: i64) -> u8 { 1 }\n\
fn show(r: Result<u8, bool>) {}\n\
fn u() {}\n\
interface Seen {\n\
    fn seen(self) -> i64\n\
}\n\
struct Eye: Seen {\n\
    v: i64\n\
    fn seen(self) -> i64 { self.v }\n\
}\n\
struct Lid: Seen {\n\
    v: i64\n\
    fn seen(self) -> i64 { self.v }\n\
}\n\
struct Stack<T> {\n\
    items: [T]\n\
    fn top(self) -> T { self.items[0] }\n\
}\n\
struct Two<A, B> {\n\
    a: A\n\
    b: B\n\
}\n\
fn dup<T>(x: T) -> Two<T, T> { Two(a: x, b: x) }\n\
fn ff<T, U>(x: Two<T, U>, y: T, z: U) -> T { y }\n\
fn none<T>() -> T? { None }\n\
fn look<T: Seen>(a: T, b: T) -> i64 { a.seen() }\n\
fn index_of<T: Eq>(xs: [T], x: T) -> i64 { 0 }\n\
fn index_last<T: Eq>(x: T, xs: [T]) -> i64 { 0 }\n\
fn keep<T: Seen>(x: T) -> [T] { [x] }\n\
struct Boxed<T> {\n\
    v: T\n\
}\n\
enum Tree<T> {\n\
    Leaf\n\
    Node(v: T)\n\
}\n";

/// The locals that a row's body can read, first in `main`.
const LOCALS: &str = "\
let c = [1].len() == 1\n\
let d = [1].len() == 2\n\
let a: u8 = 3\n\
let b: u8 = 4\n\
let o: u8? = None\n\
let m: u8? = Some(3)\n\
let n = 1\n\
let xs: [u8] = [1]\n\
let e: any Seen = Eye(v: 3)\n";

/// The program with `body` in `main`, after `HELPERS` and `LOCALS`.
fn program(body: &str) -> String {
    let mut text = format!("{HELPERS}fn main() {{\n");
    for line in LOCALS.lines().chain(body.lines()) {
        text.push_str("    ");
        text.push_str(line);
        text.push('\n');
    }
    text.push_str("}\n");
    text
}

/// A position of a row's body, (line, column), as a position of its `program`.
fn in_program((line, col): (usize, usize)) -> (usize, usize) {
    (
        HELPERS.lines().count() + 1 + LOCALS.lines().count() + line,
        col + 4,
    )
}

/// The type of the value of `let x` in `main` of `src`, read from the `tir` and printed as the
/// diagnostics print it. `src` checks with no error.
pub(in crate::check) fn join_type(src: &str) -> String {
    let (program, ds) = checked_with(PRELUDE_SOURCE, src);
    let found = errors(src, &ds);
    assert!(found.is_empty(), "{found:?}");
    let program = program.expect("a program with no error should have a `tir`");
    let main = (program.functions.iter())
        .find(|f| f.name == "main")
        .expect("the program should define `main`");
    let ty = (main.body.stmts.iter())
        .find_map(|stmt| match stmt {
            tir::Stmt::Let { local, .. } => {
                let local = &main.locals[local.0];
                (local.name.as_deref() == Some("x")).then_some(local.ty)
            }
            _ => None,
        })
        .expect("`main` should have `let x`");
    program.type_name(ty)
}

/// Each body is valid in its `program`, and its `x` has the type of the row. Asserts once at the
/// end, so that a red run lists each red row.
fn type_rows(rows: &[(&str, &str)]) {
    let red: Vec<String> = (rows.iter())
        .filter_map(|&(body, wanted)| {
            let src = program(body);
            let (_, ds) = checked_with(PRELUDE_SOURCE, &src);
            let found = errors(&src, &ds);
            if !found.is_empty() {
                return Some(format!("row `{body}`: found {found:?}, wanted no error"));
            }
            let ty = join_type(&src);
            (ty != wanted).then(|| format!("row `{body}`: type `{ty}`, wanted `{wanted}`"))
        })
        .collect();
    assert!(
        red.is_empty(),
        "{} red rows:\n{}",
        red.len(),
        red.join("\n")
    );
}

/// `error_rows` on each body in its `program`, with each position given in the body.
fn body_rows(rows: &[(&str, &[Wanted])]) {
    let rows: Vec<ErrorRow<'_>> = (rows.iter())
        .map(|&(body, wanted)| {
            let wanted = (wanted.iter())
                .map(|&(code, at)| (code, in_program(at)))
                .collect();
            (body, program(body), wanted)
        })
        .collect();
    error_rows(&rows);
}

#[test]
fn join_should_keep_the_result_that_main_gives() {
    // GIVEN joins that `main` checks in each order today
    // WHEN each is checked
    // THEN each keeps the result of `main`: no error and the same type, or the same errors
    type_rows(&[
        ("let x = pick(if c { 1 } else { 2 }, a)", "u8"),
        ("let x = pick(a, if c { 1 } else { 2 })", "u8"),
        ("let x = pick(a, loop { break 1 })", "u8"),
        ("let x = pick(1 + 2, a)", "u8"),
        ("let x = pick(a, 1 + 2)", "u8"),
        ("let x = pick([1, 2], [a])", "[u8]"),
        ("let x = pick([a], [1, 2])", "[u8]"),
        ("let x = pick(o ?? 1, a)", "u8"),
        ("let x = pick(a, o ?? 1)", "u8"),
        ("let x = pick(match n { _ => 1 }, a)", "u8"),
        ("let x = pick(a, match n { _ => 1 })", "u8"),
        ("let x = if c { 1 + 2 } else { a }", "u8"),
        ("let x = if c { a } else { 1 + 2 }", "u8"),
        ("let x = [1 + 2, a]", "[u8]"),
        ("let x = [a, 1 + 2]", "[u8]"),
        ("let x = loop { if c { break a }; break 1 + 2 }", "u8"),
        (
            "let x: Result<[u8], bool> = Ok(dbg([7]))",
            "Result<[u8], bool>",
        ),
        ("let x: [u8] = id([5; 2])", "[u8]"),
        ("let x: u8 = id(1 + loop { break 2 })", "u8"),
        ("let x: u8 = id(loop { break 2 } + 1)", "u8"),
        ("let x: [u8] = id([loop { break 13 }, 2])", "[u8]"),
        ("let x: [u8] = id([2, loop { break 13 }])", "[u8]"),
        ("let x: [u8] = id([1, 2])", "[u8]"),
        ("let x = pick(Some(1 + 2), o)", "u8?"),
        ("let x = pick(o, Some(1 + 2))", "u8?"),
        ("let x = pick(id(1 + 2), a)", "u8"),
        ("let x = pick(a, id(1 + 2))", "u8"),
        ("let x = pick({ 1 + 2 }, a)", "u8"),
        ("let x = pick(a, { 1 + 2 })", "u8"),
        ("let x = pick(if c { 1 + 2 } else { 3 }, a)", "u8"),
        ("let x = pick(a, if c { 1 + 2 } else { 3 })", "u8"),
        ("let x: u8 = if c { 1 + 2 } else { 3 }", "u8"),
        ("let x: u8 = if c { 3 } else { 1 + 2 }", "u8"),
        ("let x: [u8] = [1 + 2, 3]", "[u8]"),
        ("let x: [u8] = [3, 1 + 2]", "[u8]"),
        ("let x: u8 = loop { if c { break 1 + 2 }; break 3 }", "u8"),
        ("let x: u8 = loop { if c { break 3 }; break 1 + 2 }", "u8"),
        (
            "let x = loop { let y: u8 = if c { if d { break 300 } else { 2 } } else { 3 }; print(y); break 1 }",
            "i64",
        ),
    ]);
    body_rows(&[
        ("let x = loop { if c { break }; break return }", &[]),
        (
            "let x = loop { if c { break }; break if d { u() } else { u() } }",
            &[],
        ),
        ("show(Ok(if c { 3 } else { 4 }))", &[]),
        ("show(Ok(if c { 4 } else { 3 }))", &[]),
    ]);
}

#[test]
fn member_at_each_value_position_should_stay_open() {
    // GIVEN a literal at each value position inside an argument of `pick`, beside a `u8`
    // WHEN each is checked
    // THEN the literal takes `u8` from the other argument
    type_rows(&[
        ("let x = pick(a, { 1 })", "u8"),
        ("let x = pick(a, if c { 1 } else { 2 })", "u8"),
        ("let x = pick(a, match n { _ => 1 })", "u8"),
        ("let x = pick([a], [1])", "[u8]"),
        ("let x = pick([a], [1; 2])", "[u8]"),
        ("let x = pick(m, Some(1))", "u8?"),
        ("let x = pick(a, loop { break 1 })", "u8"),
        ("let x = pick(a, dbg(1))", "u8"),
        ("let x = pick(a, id(1))", "u8"),
    ]);
}

#[test]
fn generic_call_in_a_join_should_check_in_each_order() {
    // GIVEN joins that hold a generic call, each in both orders
    // WHEN each is checked
    // THEN each is valid with the type that `main` gives
    type_rows(&[
        ("let x = if c { a } else { id(1) }", "u8"),
        ("let x = if c { id(1) } else { a }", "u8"),
        (
            "let x = if c { a } else { if c { id(1) } else { id(2) } }",
            "u8",
        ),
        (
            "let x = if c { if c { id(1) } else { id(2) } } else { a }",
            "u8",
        ),
        ("let x = if c { id(1) } else { id(a) }", "u8"),
        ("let x = if c { id(a) } else { id(1) }", "u8"),
        (
            "let x = if c { Tree.Leaf } else { Tree.Node(v: a) }",
            "Tree<u8>",
        ),
        (
            "let x = if c { Tree.Node(v: a) } else { Tree.Leaf }",
            "Tree<u8>",
        ),
        ("let x = if c { 1 } else { id(2) }", "i64"),
        ("let x = if c { id(2) } else { 1 }", "i64"),
        ("let x = [None, id(o)]", "[u8?]"),
        ("let x = [id(o), None]", "[u8?]"),
        ("let x = if c { [] } else { id([2]) }", "[i64]"),
        ("let x = if c { id([2]) } else { [] }", "[i64]"),
        ("let x = first(xs) ?? 0", "u8"),
        ("let x = if c { 1 } else { id(a) }", "u8"),
        ("let x = if c { id(a) } else { 1 }", "u8"),
        ("let x = [1, id(a)]", "[u8]"),
        ("let x = [id(a), 1]", "[u8]"),
        ("let x = 1 + id(a)", "u8"),
        ("let x = id(a) + 1", "u8"),
        ("let x = if c { 0 } else { pick(a, 2) }", "u8"),
        ("let x = if c { pick(a, 2) } else { 0 }", "u8"),
        ("let x = pick(id([1]), [m])", "[u8?]"),
        ("let x = pick([m], id([1]))", "[u8?]"),
        ("let x = g(a, 1)", "u8"),
        ("let x = g(1, a)", "u8"),
        ("let x = if c { None } else { last(xs) }", "u8?"),
        ("let x = if c { last(xs) } else { None }", "u8?"),
        ("let y: u8? = last(xs)\nlet x = y", "u8?"),
        ("let x = if c { None } else { big(a, 2) }", "u8?"),
        ("let x = if c { big(a, 2) } else { None }", "u8?"),
        (
            "let mut mb: u8 = 3\nlet x = if c { m } else { bump(mut mb, 1) }",
            "u8?",
        ),
        (
            "let mut mb: u8 = 3\nlet x = if c { bump(mut mb, 1) } else { m }",
            "u8?",
        ),
        (
            "let me: Eye? = Some(Eye(v: 3))\nlet x = if c { me } else { need(Eye(v: 1)) }",
            "Eye?",
        ),
        (
            "let me: Eye? = Some(Eye(v: 3))\nlet x = if c { need(Eye(v: 1)) } else { me }",
            "Eye?",
        ),
        (
            "let s = Stack<u8>(items: [1])\nlet x = if c { m } else { s.top() }",
            "u8?",
        ),
        (
            "let s = Stack<u8>(items: [1])\nlet x = if c { s.top() } else { m }",
            "u8?",
        ),
        ("let x = f(id([1]), m)", "u8?"),
        ("let x = f([m], id(1))", "u8?"),
        ("let x = if c { m } else { g(a, 1) }", "u8?"),
        ("let x = if c { g(a, 1) } else { m }", "u8?"),
        (
            "let x = if c { m } else { if d { last(xs) } else { None } }",
            "u8?",
        ),
        (
            "let x = if c { if d { last(xs) } else { None } } else { m }",
            "u8?",
        ),
        ("let t: Two<u8?, u8?> = dup(1)\nlet x = t", "Two<u8?, u8?>"),
    ]);
}

#[test]
fn generic_call_with_no_taker_should_stay_an_error() {
    // GIVEN generic calls whose type no form can take
    // WHEN each is checked
    // THEN each reports one E0032 where `main` reports it
    body_rows(&[
        ("let x = f(xs, Some(1))", &[(Code::Mismatch, (1, 15))]),
        (
            "let t: Two<u8?, u8> = Two(a: Some(a), b: a)\nlet x = if c { t } else { dup(a) }",
            &[(Code::Mismatch, (2, 27))],
        ),
        (
            "let t: Two<u8?, u8> = Two(a: Some(a), b: a)\nlet x = if c { dup(a) } else { t }",
            &[(Code::Mismatch, (2, 32))],
        ),
    ]);
}

#[test]
fn one_wrap_through_a_call_should_hold() {
    // GIVEN values that take at most one wrap through a generic call
    // WHEN each is checked
    // THEN each one with two wraps reports E0032, and the others are valid
    body_rows(&[
        ("let q: i64?? = 1", &[(Code::Mismatch, (1, 16))]),
        ("let v: i64?? = h(1, Some(2))", &[(Code::Mismatch, (1, 16))]),
        ("let s: u8?? = id(1)", &[(Code::Mismatch, (1, 15))]),
        ("let v: i64?? = Some(h(1, Some(2)))", &[]),
        ("let v: i64?? = h(1, Some(Some(2)))", &[]),
    ]);
}

#[test]
fn join_should_give_the_same_result_in_each_order() {
    // GIVEN joins in both orders
    // WHEN each is checked
    // THEN both orders give the same result
    type_rows(&[
        (
            "let x = if c { e } else if d { Eye(v: 1) } else { Lid(v: 2) }",
            "any Seen",
        ),
        (
            "let x = if c { Eye(v: 1) } else if d { Lid(v: 2) } else { e }",
            "any Seen",
        ),
        ("let x = if c { a } else { panic(\"no\") }", "u8"),
        ("let x = if c { panic(\"no\") } else { a }", "u8"),
    ]);
    body_rows(&[
        (
            "let mi: i64? = Some(5)\nlet xi: [i64] = [1]\nlet x = if c { [mi] } else { xi }",
            &[(Code::Mismatch, (3, 30))],
        ),
        (
            "let mi: i64? = Some(5)\nlet xi: [i64] = [1]\nlet x = if c { xi } else { [mi] }",
            &[(Code::Mismatch, (3, 29))],
        ),
    ]);
}

#[test]
fn poisoned_member_should_report_nothing_more() {
    // GIVEN joins with a failed member, and loops whose body has an error
    // WHEN each is checked
    // THEN each gives the errors that `main` gives, and nothing more for the failed member
    body_rows(&[
        (
            "let x = if c { nope } else { 1 }\nlet y: u8 = x",
            &[(Code::BindingNotFound, (1, 16))],
        ),
        (
            "let x = if c { nope } else { None }\nprint(x ?? 0)",
            &[(Code::BindingNotFound, (1, 16))],
        ),
        (
            "let x = if c { bad(nope) } else { 1 }\nlet y: string = x",
            &[(Code::BindingNotFound, (1, 20)), (Code::Mismatch, (2, 17))],
        ),
        (
            "let x = if c { bad(nope) } else { bad(nope2) }\nlet y: string = x",
            &[
                (Code::BindingNotFound, (1, 20)),
                (Code::BindingNotFound, (1, 39)),
                (Code::Mismatch, (2, 17)),
            ],
        ),
        (
            "let x = loop { let z: string = 1; break a }",
            &[(Code::Mismatch, (1, 32))],
        ),
        (
            "let x = loop { if c { break bad(nope) }; break 1 }\nlet y: string = x",
            &[(Code::BindingNotFound, (1, 33)), (Code::Mismatch, (2, 17))],
        ),
        (
            "let x = if c { loop { prnt(\"a\"); break a } } else { 1 }\nlet y: u8 = x",
            &[(Code::FunctionNotFound, (1, 23))],
        ),
        (
            "let x = loop { prnt(\"a\"); break None }\nlet y = x ?? 0",
            &[
                (Code::FunctionNotFound, (1, 16)),
                (Code::CannotInfer, (1, 33)),
            ],
        ),
        (
            "let x = loop { prnt(\"a\"); break if d { a } else { o } }\nlet y: string = x",
            &[(Code::FunctionNotFound, (1, 16)), (Code::Mismatch, (2, 17))],
        ),
        (
            "let x = loop { prnt(\"a\"); if c { break [o] }; break id([1]) }\nlet y: string = x",
            &[(Code::FunctionNotFound, (1, 16)), (Code::Mismatch, (2, 17))],
        ),
        (
            "let x = loop { prnt(\"a\"); if c { break if d { a } else { 2 } }; break 1 }\nlet y: string = x",
            &[(Code::FunctionNotFound, (1, 16)), (Code::Mismatch, (2, 17))],
        ),
        (
            "let x = loop { prnt(\"a\"); break if d { None } else { None } }\nlet y = x ?? 0",
            &[
                (Code::FunctionNotFound, (1, 16)),
                (Code::CannotInfer, (1, 40)),
            ],
        ),
        (
            "let x = loop { prnt(\"a\"); if c { break }; break None }",
            &[(Code::FunctionNotFound, (1, 16)), (Code::NoValue, (1, 34))],
        ),
        (
            "let x = [loop { prnt(\"a\"); break a }, 1]\nlet y: [u8] = x",
            &[(Code::FunctionNotFound, (1, 17))],
        ),
        (
            "let x = pick(loop { prnt(\"a\"); break a }, 1)\nlet y: u8 = x",
            &[(Code::FunctionNotFound, (1, 21))],
        ),
        (
            "let x = [loop { prnt(\"a\"); break a }, o]\nlet y: [u8?] = x",
            &[(Code::FunctionNotFound, (1, 17))],
        ),
        (
            "let x = if c { if d { prnt(\"a\"); a } else { a } } else { 1 }\nlet y: u8 = x",
            &[(Code::FunctionNotFound, (1, 23))],
        ),
    ]);
}

#[test]
fn bound_breaking_member_of_a_closed_call_should_give_only_its_bound_error() {
    // GIVEN closed generic calls whose argument of a closed type breaks the bound of its type
    // parameter. Another argument at that parameter stands beside it. The call stands alone,
    // beside `==` and in an `if`
    // WHEN each is checked
    // THEN each gives only E0154 at that argument: the call's join adds no conflict, and the
    // poisoned call adds nothing to the join around it
    body_rows(&[
        ("let x = big(Eye(v: 1), a)", &[(Code::NotConform, (1, 13))]),
        ("let x = big(a, Eye(v: 1))", &[(Code::NotConform, (1, 16))]),
        ("let x = look(a, Eye(v: 2))", &[(Code::NotConform, (1, 14))]),
        ("let x = big(Eye(v: 1), 2)", &[(Code::NotConform, (1, 13))]),
        (
            "let x = big(Boxed(v: 1), 2)",
            &[(Code::NotConform, (1, 13))],
        ),
        (
            "let x = a == big(Eye(v: 1), a)",
            &[(Code::NotConform, (1, 18))],
        ),
        (
            "let x = if c { big(Eye(v: 1), a) } else { a }",
            &[(Code::NotConform, (1, 20))],
        ),
    ]);
}

#[test]
fn open_call_in_a_call_join_should_take_the_candidate() {
    // GIVEN `pick` of a `Two<u8?, u8?>` and `dup(1)`, in each order
    // WHEN each is checked
    // THEN each is valid: `dup` takes `T = u8?` from the candidate, and `1` wraps inside it
    type_rows(&[
        (
            "let tt: Two<u8?, u8?> = Two(a: None, b: None)\nlet x = pick(tt, dup(1))",
            "Two<u8?, u8?>",
        ),
        (
            "let tt: Two<u8?, u8?> = Two(a: None, b: None)\nlet x = pick(dup(1), tt)",
            "Two<u8?, u8?>",
        ),
    ]);
}

#[test]
fn open_call_whose_candidate_breaks_its_bound_should_stay_closed() {
    // GIVEN `pick` of a `[Eye?]` and `keep(Eye(v: 1))`, in each order: at `[Eye?]`, `keep` would
    //       take `T = Eye?`, which breaks `T: Seen`
    // WHEN each is checked
    // THEN each reports one E0032 at `pick`
    body_rows(&[
        (
            "let ys: [Eye?] = [None]\nlet x = pick(ys, keep(Eye(v: 1)))",
            &[(Code::Mismatch, (2, 9))],
        ),
        (
            "let ys: [Eye?] = [None]\nlet x = pick(keep(Eye(v: 1)), ys)",
            &[(Code::Mismatch, (2, 9))],
        ),
    ]);
}

#[test]
fn poisoned_member_should_poison_its_type_parameter() {
    // GIVEN `f(nope, [none_of()], 1)` with `fn f<T>(a: T, d: [T?], e: T?)`: `nope` names no
    //       binding
    // WHEN it is checked
    // THEN it reports only E0033 at `nope`: `T` is poisoned, and `[none_of()]` adds no E0050
    let src = "fn none_of<X>() -> X? { None }\nfn f<T>(a: T, d: [T?], e: T?) {}\nfn main() {\n    \
               f(nope, [none_of()], 1)\n}\n";
    error_rows(&[(
        "call",
        src.to_owned(),
        vec![(Code::BindingNotFound, (4, 7))],
    )]);
}

#[test]
fn bare_break_in_a_member_loop_should_keep_its_report() {
    // GIVEN loops with a bare `break` beside a `break` with a value, each a member of a join
    // WHEN each is checked
    // THEN each reports E0043 at the bare `break`, as `main` does, and a loop with no value
    //      reports E0043 at the loop
    body_rows(&[
        (
            "let x = if c { loop { if d { break }; break 1 } } else { a }",
            &[(Code::NoValue, (1, 30))],
        ),
        (
            "let x = pick(loop { if d { break }; break 1 }, a)",
            &[(Code::NoValue, (1, 28))],
        ),
        (
            "let x = [loop { if d { break }; break 1 }, a]",
            &[(Code::NoValue, (1, 24))],
        ),
        (
            "let x: u8 = if c { loop { if d { break }; break 1 } } else { 2 }",
            &[(Code::NoValue, (1, 34))],
        ),
        (
            "let x: u8? = loop { if d { break }; break None }",
            &[(Code::NoValue, (1, 28))],
        ),
        (
            "let x = [loop { if d { break }; break 1 }, loop { if c { break }; break a }]",
            &[(Code::NoValue, (1, 24)), (Code::NoValue, (1, 58))],
        ),
        (
            "let x = if c { a } else { outer: loop { loop { if d { break outer }; break outer 1 } } }",
            &[(Code::NoValue, (1, 55))],
        ),
        ("let x: u8 = loop { break }", &[(Code::NoValue, (1, 13))]),
        ("let x = [loop { break }, 1]", &[(Code::NoValue, (1, 10))]),
    ]);
}

#[test]
fn literal_check_should_wait_for_the_join() {
    // GIVEN literals that do not fit the type of their join
    // WHEN each is checked
    // THEN each reports E0040 or E0042 at the literal, in both orders
    body_rows(&[
        (
            "let x: u8 = loop { prnt(\"a\"); if c { break 300 }; break a }",
            &[
                (Code::FunctionNotFound, (1, 20)),
                (Code::IntegerDoesNotFit, (1, 44)),
            ],
        ),
        (
            "let x: u8 = loop { prnt(\"a\"); break -1 }",
            &[
                (Code::FunctionNotFound, (1, 20)),
                (Code::NegatedU8, (1, 37)),
            ],
        ),
        ("let x = [a, 300]", &[(Code::IntegerDoesNotFit, (1, 13))]),
        ("let x = [300, a]", &[(Code::IntegerDoesNotFit, (1, 10))]),
        (
            "let x = if c { a } else { -1 }",
            &[(Code::NegatedU8, (1, 27))],
        ),
        (
            "let x = if c { -1 } else { a }",
            &[(Code::NegatedU8, (1, 16))],
        ),
        (
            "let x = if c { a } else { 256 / 2 }",
            &[(Code::IntegerDoesNotFit, (1, 27))],
        ),
        (
            "let x = if c { 256 / 2 } else { a }",
            &[(Code::IntegerDoesNotFit, (1, 16))],
        ),
    ]);
}

#[test]
fn literal_alone_should_be_i64_at_its_let() {
    // GIVEN a join of two literals, and a `let` with a `u8` type that reads it
    // WHEN it is checked
    // THEN the join is `i64`, and the `let` reports E0032 at `x`
    type_rows(&[("let x = if c { 1 } else { 2 }", "i64")]);
    body_rows(&[(
        "let x = if c { 1 } else { 2 }\nlet y: u8 = x",
        &[(Code::Mismatch, (2, 13))],
    )]);
}

#[test]
fn open_value_in_a_failed_context_should_fail_quietly() {
    // GIVEN a `None` of type `Any?`, whose `Any` is open, in a context that failed
    // WHEN `close` closes it
    // THEN it fails with no type and no report, so no open variable leaves the checker
    let src = "fn main() {\n    None\n}\n";
    let span = Span::new(16, 20);
    with_checker(src, |c| {
        let ty = Type::optional(c.table.fresh(Kind::Any, span));
        let value = tir::Expr::new(tir::ExprKind::None, ty, span);
        let e = ast::Expr {
            kind: ast::ExprKind::NoneLit,
            span,
        };
        let (closed, mismatched) = c.close(&e, Ok(value), Expectation::Poisoned);
        assert!(!mismatched);
        assert!(matches!(closed, Err(Poisoned { ty: None, .. })));
        assert!(c.diagnostics.is_empty(), "the failed context has reported");
        assert_eq!(c.table.unsettled(ty), [], "the `Any` set is poisoned");
    });
}

#[test]
fn poisoned_open_value_in_a_failed_context_should_fail_quietly() {
    // GIVEN a `None` whose `Any` an earlier failed use has poisoned, in a context that failed
    // WHEN `close` closes it
    // THEN it fails with no type, as the variable is still open
    let src = "fn main() {\n    None\n}\n";
    let span = Span::new(16, 20);
    with_checker(src, |c| {
        let ty = Type::optional(c.table.fresh(Kind::Any, span));
        c.table.poison(ty);
        let value = tir::Expr::new(tir::ExprKind::None, ty, span);
        let e = ast::Expr {
            kind: ast::ExprKind::NoneLit,
            span,
        };
        let (closed, _) = c.close(&e, Ok(value), Expectation::Poisoned);
        assert!(matches!(closed, Err(Poisoned { ty: None, .. })));
    });
}

#[test]
fn open_set_shared_with_a_failed_context_should_close_quietly_under_no_type() {
    // GIVEN two `None`s that share one open `Any`, and the error of a context that failed
    // WHEN the first is closed under `Poisoned`, the second under no type, and the function's
    // table is written back
    // THEN the first fails with no type, and the second keeps its poisoned `Any` with no E0050
    // AND the write-back finds the error that covers the open variable
    let src = "fn main() {\n    None\n    None\n}\n";
    let (first, second) = (Span::new(16, 20), Span::new(25, 29));
    let program = parse_ok(src);
    let mut checker = Checker::new(&program, src, "");
    let closed = checker.with_table(|c| {
        let any = c.table.fresh(Kind::Any, first);
        let none = |span| {
            let e = ast::Expr {
                kind: ast::ExprKind::NoneLit,
                span,
            };
            (
                e,
                tir::Expr::new(tir::ExprKind::None, Type::optional(any), span),
            )
        };
        c.diagnostics.error(DiagnosticKind::NegatedU8 {
            at: first,
            convert: None,
        });
        let (e, value) = none(first);
        let (closed, _) = c.close(&e, Ok(value), Expectation::Poisoned);
        assert!(matches!(closed, Err(Poisoned { ty: None, .. })));
        let (e, value) = none(second);
        c.close(&e, Ok(value), Expectation::None).0
    });
    assert!(closed.is_ok());
    let codes: Vec<Code> = checker.diagnostics.iter().map(Diagnostic::code).collect();
    assert_eq!(codes, [Code::NegatedU8]);
}

#[test]
fn join_after_a_conflict_should_be_untyped() {
    // GIVEN a join of a `u8` and an `i64`, and a `let` with a `u8` type that reads it
    // WHEN it is checked
    // THEN only the join reports E0032, and the `let` reports nothing
    body_rows(&[(
        "let bi: i64 = 4\nlet x = if c { a } else { bi }\nlet y: u8 = x",
        &[(Code::Mismatch, (2, 27))],
    )]);
}

#[test]
fn coalesce_should_join_its_right_side_with_the_payload() {
    // GIVEN `o ?? 1` with `o: u8?`, and a `let` with a `u8` type that reads it
    // WHEN it is checked
    // THEN it is valid
    body_rows(&[("let x = o ?? 1\nlet y: u8 = x", &[])]);
}

#[test]
fn call_under_a_required_type_should_keep_the_directed_report() {
    // GIVEN a generic call whose two arguments for `T` differ, under a required `u8` and `u8?`
    // WHEN each is checked
    // THEN each reports one E0032 at `"s"`, with a label at `a`, which gave `T` its type
    for (body, primary, label) in [
        ("let y: u8 = pick(a, \"s\")", (1, 21), (1, 18)),
        ("let y: u8? = pick(a, \"s\")", (1, 22), (1, 19)),
    ] {
        let src = program(body);
        let (_, ds) = checked_with(PRELUDE_SOURCE, &src);
        let found: Vec<Found> = (ds.iter())
            .filter(|d| d.code().severity() == Severity::Error)
            .map(|d| {
                let secondary = d.secondary().iter().map(|(s, _)| at(&src, *s)).collect();
                (d.code(), at(&src, d.span()), secondary)
            })
            .collect();
        let wanted = vec![(Code::Mismatch, in_program(primary), vec![in_program(label)])];
        assert_eq!(found, wanted, "row `{body}`");
    }
}

#[test]
fn required_type_should_win_over_the_members() {
    // GIVEN `let x: f64 = if c { 1 } else { 2 }`
    // WHEN it is checked
    // THEN it is valid, and both literals are `f64` in the `tir`
    let src = program("let x: f64 = if c { 1 } else { 2 }");
    let (program, ds) = checked_with(PRELUDE_SOURCE, &src);
    let found = errors(&src, &ds);
    assert!(found.is_empty(), "{found:?}");
    let printed = program.expect("no errors").to_string();
    assert!(
        printed.contains("(let x#")
            && printed.contains("(block 1.0)")
            && printed.contains("(block 2.0)"),
        "{printed}"
    );
}

#[test]
fn join_of_a_type_parameter_should_wrap_at_the_definition() {
    // GIVEN a generic function whose `if` joins a `T` and a `T?`
    // WHEN it is checked
    // THEN it is checked once, at its definition, and the `T` branch is wrapped
    let src = "fn cond<T>(x: T) -> bool { true }\nfn f<T>(a: T, b: T?) -> T? { if cond(a) { a } else { b } }\nfn main() {\n    print(f(1, None) ?? 0)\n}\n";
    let (program, ds) = checked_with(PRELUDE_SOURCE, src);
    let found = errors(src, &ds);
    assert!(found.is_empty(), "{found:?}");
    let printed = program.expect("no errors").to_string();
    assert!(printed.contains("(fn f<T> ((a#0 T) (b#1 T?)) T? (block (if (call cond<T> a#0) (block (wrap a#0)) (block b#1))))"), "{printed}");
}

#[test]
fn join_in_a_constant_should_use_its_own_table() {
    // GIVEN constants whose operators join a literal and a `u8` constant
    // AND a method whose `if` joins a `u8` and a `None`, and a sum of two literals in `main`
    // WHEN it is checked
    // THEN each is valid: the constants are `u8`, the method wraps the `u8`, and the sum in
    //      `main` stays `i64`
    let src = "const A: u8 = 3\nconst K: u8 = 1 + A\nconst L = A + 1\nstruct P {\n    v: u8\n    fn get(self, c: bool) -> u8? { if c { self.v } else { None } }\n}\nfn main() {\n    let x = 300 + 1\n    print(K)\n    print(L)\n    print(x)\n    print(P(v: 2).get(true) ?? 0)\n}\n";
    let (program, ds) = checked_with(PRELUDE_SOURCE, src);
    let found = errors(src, &ds);
    assert!(found.is_empty(), "{found:?}");
    let printed = program.expect("no errors").to_string();
    for part in [
        "(print 4u8) (print 4u8)",
        "(let x#0 (+ 300 1))",
        "(if c#1 (block (wrap (. self#0 v))) (block None))",
    ] {
        assert!(printed.contains(part), "{part} in {printed}");
    }
}

/// Each error of `body` in its `program`: its code, its primary position and its secondary
/// positions, in order. The positions are of the `program`.
fn reports_of(body: &str) -> Vec<Found> {
    let src = program(body);
    let (_, ds) = checked_with(PRELUDE_SOURCE, &src);
    (ds.iter())
        .filter(|d| d.code().severity() == Severity::Error)
        .map(|d| {
            let secondary = d.secondary().iter().map(|(s, _)| at(&src, *s)).collect();
            (d.code(), at(&src, d.span()), secondary)
        })
        .collect()
}

/// A row of `one_conflict_rows`: a body, the primary position of its E0032 and the position of
/// each label, all in the body.
type ConflictRow<'a> = (&'a str, (usize, usize), &'a [(usize, usize)]);

/// Each body gives one E0032 at its primary position, with a secondary position at each of
/// `labels`, in any order. Asserts once at the end.
fn one_conflict_rows(rows: &[ConflictRow<'_>]) {
    let red: Vec<String> = (rows.iter())
        .filter_map(|&(body, primary, labels)| {
            let mut found = reports_of(body);
            for (_, _, secondary) in &mut found {
                secondary.sort_unstable();
            }
            let mut labels: Vec<_> = labels.iter().copied().map(in_program).collect();
            labels.sort_unstable();
            let wanted = vec![(Code::Mismatch, in_program(primary), labels)];
            (found != wanted).then(|| format!("row `{body}`: found {found:?}, wanted {wanted:?}"))
        })
        .collect();
    assert!(
        red.is_empty(),
        "{} red rows:\n{}",
        red.len(),
        red.join("\n")
    );
}

/// The fixes of the one diagnostic of `body`, with the positions given in the body.
fn fixes_of(body: &str) -> Vec<Vec<(usize, usize, String, String)>> {
    let src = program(body);
    let (_, ds) = checked_with(PRELUDE_SOURCE, &src);
    let ds: Vec<_> = (ds.iter())
        .filter(|d| d.code().severity() == Severity::Error)
        .collect();
    assert_eq!(ds.len(), 1, "row `{body}`: {ds:?}");
    let base = in_program((0, 0));
    fixes(&src, ds[0])
        .into_iter()
        .map(|(_, edits)| {
            (edits.into_iter())
                .map(|(line, col, old, new)| (line - base.0, col - base.1, old.to_owned(), new))
                .collect()
        })
        .collect()
}

#[test]
fn generic_call_should_join_the_arguments_of_each_type_parameter() {
    // GIVEN generic calls whose arguments for one `T` have different types, in each order
    // WHEN each is checked
    // THEN each is valid with the type that one of the arguments gives
    type_rows(&[
        ("let x = look(Eye(v: 1), e)", "i64"),
        ("let x = look(e, Eye(v: 1))", "i64"),
        ("let x: u8? = pick(a, None)", "u8?"),
        ("let x: u8? = pick(None, a)", "u8?"),
        ("let x = pick(a, None)", "u8?"),
        ("let x = pick(None, a)", "u8?"),
        ("let x = f([Some(1)], a)", "u8?"),
        ("let x = f([a], Some(1))", "u8?"),
        ("let x = pick(loop { break 1 }, a)", "u8"),
    ]);
}

#[test]
fn inner_call_at_a_place_of_two_parameters_should_stay_closed() {
    // GIVEN `ff(dup(1), m, 2)`, whose `dup(1)` is at `Two<T, U>`, in each order of `m` and `2`
    // WHEN each is checked
    // THEN each reports one E0032 at the call: `dup(1)` gives `T` the type `i64`
    body_rows(&[
        ("let x = ff(dup(1), m, 2)", &[(Code::Mismatch, (1, 9))]),
        ("let x = ff(dup(1), 2, m)", &[(Code::Mismatch, (1, 9))]),
    ]);
}

#[test]
fn part_of_an_argument_should_get_the_argument_label() {
    // GIVEN `index_of(names, nn)`, whose `names: [string]` is at `[T]` and `nn: i64` at `T`, in
    //       each order of the parameters
    // WHEN each is checked
    // THEN each reports one E0032 at the call, with a label at `names` and at `nn`
    one_conflict_rows(&[
        (
            "let names: [string] = [\"a\"]\nlet nn: i64 = 3\nprint(index_of(names, nn))",
            (3, 7),
            &[(3, 16), (3, 23)],
        ),
        (
            "let names: [string] = [\"a\"]\nlet nn: i64 = 3\nprint(index_last(nn, names))",
            (3, 7),
            &[(3, 18), (3, 22)],
        ),
    ]);
}

#[test]
fn call_with_a_failed_join_should_report_once() {
    // GIVEN `pick(a, "s")` as a branch of an `if`, beside a literal, in each order
    // WHEN each is checked
    // THEN each reports one E0032 at `pick`, with a label at `a` and at `"s"`, and the `if` has
    //      no report
    one_conflict_rows(&[
        (
            "let x = if c { pick(a, \"s\") } else { 1 }",
            (1, 16),
            &[(1, 21), (1, 24)],
        ),
        (
            "let x = if c { 1 } else { pick(a, \"s\") }",
            (1, 27),
            &[(1, 32), (1, 35)],
        ),
    ]);
}

#[test]
fn call_under_a_required_type_should_report_once_when_the_arguments_agree() {
    // GIVEN `pick(a, b)` with `a, b: u8`, under a required `string`
    // WHEN it is checked
    // THEN it reports one E0032 at the whole call
    body_rows(&[("let x: string = pick(a, b)", &[(Code::Mismatch, (1, 17))])]);
}

#[test]
fn call_with_an_unknown_and_a_never_argument_should_report_only_the_unknown_name() {
    // GIVEN `pick(nope, panic("x"))`: `nope` names no binding, and `panic("x")` is `never`
    // WHEN it is checked
    // THEN it reports only E0033 at `nope`, and no E0138 for `never`
    body_rows(&[(
        "let x = pick(nope, panic(\"x\"))",
        &[(Code::BindingNotFound, (1, 14))],
    )]);
}

#[test]
fn shared_none_under_a_failed_parameter_type_should_report_only_the_type() {
    // GIVEN two `None`s of one `if` at a parameter, and at a field, whose type names no type
    // WHEN each is checked
    // THEN each reports only E0030 at the type
    let call = "fn f(x: Nope) {}\nfn main() {\n    let c = [1].len() == 1\n    \
                f(if c { None } else { None })\n}\n";
    let build = "struct P { v: Nope }\nfn main() {\n    let c = [1].len() == 1\n    \
                 let p = P(v: if c { None } else { None })\n}\n";
    error_rows(&[
        ("call", call.to_owned(), vec![(Code::UnknownType, (1, 9))]),
        (
            "build",
            build.to_owned(),
            vec![(Code::UnknownType, (1, 15))],
        ),
    ]);
}

#[test]
fn if_and_match_should_join_in_each_order() {
    // GIVEN `if` and `match` joins whose members need a call, a wrap or a nested join, in each
    //       order
    // WHEN each is checked
    // THEN each is valid with the same type in each order
    type_rows(&[
        ("let x = if c { [a] } else { [id(1)] }", "[u8]"),
        ("let x = if c { [id(1)] } else { [a] }", "[u8]"),
        ("let x = if c { m } else { Some(id(1)) }", "u8?"),
        ("let x = if c { Some(id(1)) } else { m }", "u8?"),
        ("let x = if c { a } else { id(1) + 0 }", "u8"),
        ("let x = if c { id(1) + 0 } else { a }", "u8"),
        ("let x = if c { Some(0) } else { first(xs) }", "u8?"),
        ("let x = if c { first(xs) } else { Some(0) }", "u8?"),
        (
            "let x = if c { Tree.Node(v: 1) } else { Tree.Node(v: a) }",
            "Tree<u8>",
        ),
        (
            "let x = if c { Tree.Node(v: a) } else { Tree.Node(v: 1) }",
            "Tree<u8>",
        ),
        ("let x = if c { [m] } else { id([1]) }", "[u8?]"),
        ("let x = if c { id([1]) } else { [m] }", "[u8?]"),
        (
            "let x = if c { e } else { pick(Eye(v: 1), Lid(v: 2)) }",
            "any Seen",
        ),
        (
            "let x = if c { pick(Eye(v: 1), Lid(v: 2)) } else { e }",
            "any Seen",
        ),
        (
            "let x = if c { Boxed(v: 1) } else { Boxed(v: a) }",
            "Boxed<u8>",
        ),
        (
            "let x = if c { Boxed(v: a) } else { Boxed(v: 1) }",
            "Boxed<u8>",
        ),
        (
            "let x = if c { Ok(1) } else { Err(\"s\") }",
            "Result<i64, string>",
        ),
        (
            "let x = if c { Err(\"s\") } else { Ok(1) }",
            "Result<i64, string>",
        ),
        ("let x = if c { [None] } else { [last(xs)] }", "[u8?]"),
        ("let x = if c { [last(xs)] } else { [None] }", "[u8?]"),
        (
            "let tt: Two<u8?, u8?> = Two(a: None, b: None)\nlet x = if c { dup(1) } else { tt }",
            "Two<u8?, u8?>",
        ),
        (
            "let tt: Two<u8?, u8?> = Two(a: None, b: None)\nlet x = if c { tt } else { dup(1) }",
            "Two<u8?, u8?>",
        ),
        (
            "let ys: [u8?] = [Some(1)]\nlet x = if c { [1; 3] } else { ys }",
            "[u8?]",
        ),
        (
            "let ys: [u8?] = [Some(1)]\nlet x = if c { ys } else { [1; 3] }",
            "[u8?]",
        ),
        (
            "let x = match c { true => e; false => if d { Eye(v: 1) } else { Lid(v: 2) } }",
            "any Seen",
        ),
        (
            "let x = match c { true => if d { Eye(v: 1) } else { Lid(v: 2) }; false => e }",
            "any Seen",
        ),
        (
            "let x = if c { Some(e) } else { Some(if d { Eye(v: 1) } else { Lid(v: 2) }) }",
            "(any Seen)?",
        ),
        (
            "let x = if c { Some(if d { Eye(v: 1) } else { Lid(v: 2) }) } else { Some(e) }",
            "(any Seen)?",
        ),
    ]);
}

#[test]
fn conflict_should_name_each_member() {
    // GIVEN `if` joins with no candidate, in each order
    // WHEN each is checked
    // THEN each reports one E0032 at the `if`, with a label at each member
    one_conflict_rows(&[
        (
            "let x = if c { Eye(v: 1) } else { Lid(v: 2) }",
            (1, 9),
            &[(1, 16), (1, 35)],
        ),
        (
            "let x = if c { Lid(v: 2) } else { Eye(v: 1) }",
            (1, 9),
            &[(1, 16), (1, 35)],
        ),
        (
            "let bi: i64 = 4\nlet x = if c { a } else { bi }",
            (2, 9),
            &[(2, 16), (2, 27)],
        ),
        (
            "let bi: i64 = 4\nlet x = if c { bi } else { a }",
            (2, 9),
            &[(2, 16), (2, 28)],
        ),
        (
            "let x = if c { 1 } else { \"s\" }",
            (1, 9),
            &[(1, 16), (1, 27)],
        ),
        (
            "let x = if c { \"s\" } else { 1 }",
            (1, 9),
            &[(1, 16), (1, 29)],
        ),
    ]);
    // AND the conflict of a `u8` and an `i64` converts the `u8`
    assert_eq!(
        fixes_of("let bi: i64 = 4\nlet x = if c { a } else { bi }"),
        vec![vec![(2, 16, "a".to_owned(), "i64(a)".to_owned())]]
    );
}

#[test]
fn one_interface_should_give_the_fix() {
    // GIVEN `let x = if c { Eye(v: 1) } else { Lid(v: 2) }`, whose members both fit `Seen`
    // WHEN it is checked
    // THEN the one fix writes `: any Seen` after `x`
    assert_eq!(
        fixes_of("let x = if c { Eye(v: 1) } else { Lid(v: 2) }"),
        vec![vec![(1, 6, String::new(), ": any Seen".to_owned())]]
    );
}

#[test]
fn no_let_should_give_the_interface_help() {
    // GIVEN `[Eye(v: 1), Lid(v: 2)]` with no `let`
    // WHEN it is checked
    // THEN it reports one E0032 with a help and no fix
    let src = program("print([Eye(v: 1), Lid(v: 2)].len())");
    let (_, ds) = checked_with(PRELUDE_SOURCE, &src);
    let found: Vec<_> = (ds.iter())
        .filter(|d| d.code().severity() == Severity::Error)
        .map(|d| (d.code(), d.help().is_some(), d.fixes().len()))
        .collect();
    assert_eq!(found, vec![(Code::Mismatch, true, 0)]);
}

#[test]
fn array_elements_should_join_in_each_order() {
    // GIVEN arrays whose elements need a wrap, a conversion or a nested join, in each order
    // WHEN each is checked
    // THEN each is valid with the same type in each order
    type_rows(&[
        ("let x = if c { [a] } else { [1, 2] }", "[u8]"),
        ("let x = if c { [1, 2] } else { [a] }", "[u8]"),
        (
            "let mi: i64? = Some(5)\nlet x = if c { [mi] } else { [1] }",
            "[i64?]",
        ),
        (
            "let mi: i64? = Some(5)\nlet x = if c { [1] } else { [mi] }",
            "[i64?]",
        ),
        (
            "let mi: i64? = Some(5)\nlet x = match c { true => [mi]; false => [1] }",
            "[i64?]",
        ),
        (
            "let mi: i64? = Some(5)\nlet x = match c { true => [1]; false => [mi] }",
            "[i64?]",
        ),
        (
            "let mi: i64? = Some(5)\nlet x = if let Some(v) = mi { print(v); [mi] } else { [1] }",
            "[i64?]",
        ),
        (
            "let mi: i64? = Some(5)\nlet x = if let Some(v) = mi { print(v); [1] } else { [mi] }",
            "[i64?]",
        ),
        ("let x = if c { [None; 2] } else { [Some(1)] }", "[i64?]"),
        ("let x = if c { [Some(1)] } else { [None; 2] }", "[i64?]"),
        ("let x = [e, Some(Eye(v: 1))]", "[(any Seen)?]"),
        ("let x = [Some(Eye(v: 1)), e]", "[(any Seen)?]"),
        ("let x = [Some(1), a]", "[u8?]"),
        ("let x = [a, Some(1)]", "[u8?]"),
        (
            "let x = [e, if d { Eye(v: 1) } else { Lid(v: 2) }]",
            "[any Seen]",
        ),
        (
            "let x = [if d { Eye(v: 1) } else { Lid(v: 2) }, e]",
            "[any Seen]",
        ),
        (
            "let x = if c { [e] } else { [if d { Eye(v: 1) } else { Lid(v: 2) }] }",
            "[any Seen]",
        ),
        (
            "let x = if c { [if d { Eye(v: 1) } else { Lid(v: 2) }] } else { [e] }",
            "[any Seen]",
        ),
    ]);
}

#[test]
fn operator_operands_should_join_in_each_order() {
    // GIVEN operators, comparisons and `??` whose sides need a wrap or a conversion, in each
    //       order
    // WHEN each is checked
    // THEN each is valid with the same type in each order
    type_rows(&[
        ("let x = loop { break 1 } + a", "u8"),
        ("let x = a + loop { break 1 }", "u8"),
        ("let x = Some(1) == o", "bool"),
        ("let x = o == Some(1)", "bool"),
        ("let x = [1] == [a]", "bool"),
        ("let x = [a] == [1]", "bool"),
        ("let x = o ?? Some(1)", "u8?"),
        ("let x = Some(1) ?? o", "u8?"),
    ]);
    // AND `??` with a right side that is not the payload of its left side reports one E0032
    body_rows(&[("let x = o ?? \"s\"", &[(Code::Mismatch, (1, 11))])]);
}

#[test]
fn compound_fix_should_convert_only_the_right_side() {
    // GIVEN `+=` and `??` whose two sides have different types
    // WHEN each is checked
    // THEN each reports one E0032, and only a whole right side gets a fix
    one_conflict_rows(&[(
        "let mut s: u8 = 0\nlet bi: i64 = 300\ns += bi\nprint(s)",
        (3, 3),
        &[(3, 1), (3, 6)],
    )]);
    assert_eq!(
        fixes_of("let mut s: u8 = 0\nlet bi: i64 = 300\ns += bi\nprint(s)"),
        Vec::<Vec<_>>::new()
    );
    assert_eq!(
        fixes_of("let mut s: i64 = 0\ns += a\nprint(s)"),
        vec![vec![(2, 6, "a".to_owned(), "i64(a)".to_owned())]]
    );
    assert_eq!(
        fixes_of("let bi: i64 = 300\nlet s = o ?? bi\nprint(s)"),
        Vec::<Vec<_>>::new()
    );
}

#[test]
fn break_values_should_join_in_each_order() {
    // GIVEN loops whose `break` values need a call or a literal settled by a later value, in each
    //       order
    // WHEN each is checked
    // THEN each is valid with the same type in each order
    type_rows(&[
        ("let x = loop { if c { break a }; break 1 }", "u8"),
        ("let x = loop { if c { break 1 }; break a }", "u8"),
        ("let x = loop { if c { break 1 + 2 }; break a }", "u8"),
        ("let x = loop { if c { break a }; break id(1) }", "u8"),
        ("let x = loop { if c { break id(1) }; break a }", "u8"),
        ("let x = loop { if c { break 1 }; break id(a) }", "u8"),
        ("let x = loop { if c { break id(a) }; break 1 }", "u8"),
    ]);
    // AND a loop whose body failed is `[u8?]`, which a later `let` reports
    body_rows(&[(
        "let x = loop { prnt(\"a\"); if c { break [a] }; break [o] }\nlet y: string = x",
        &[(Code::FunctionNotFound, (1, 16)), (Code::Mismatch, (2, 17))],
    )]);
}

#[test]
fn loop_under_a_required_type_should_report_once_when_the_break_values_agree() {
    // GIVEN a loop whose `break` values `a, b: u8` agree, under a required `string`
    // WHEN it is checked
    // THEN it reports one E0032 at the whole loop
    body_rows(&[(
        "let x: string = loop { if c { break a }; break b }",
        &[(Code::Mismatch, (1, 17))],
    )]);
}

#[test]
fn bare_break_should_require_a_unit_of_each_break_value() {
    // GIVEN loops with a bare `break` beside a `break` with a value that is not a `unit`
    // WHEN each is checked
    // THEN each reports one E0043 at the bare `break`, and nothing for the values
    body_rows(&[
        (
            "let x = loop { if c { break }; break None }",
            &[(Code::NoValue, (1, 23))],
        ),
        (
            "let x = if c { loop { if d { break }; break None } } else { Some(u()) }",
            &[(Code::NoValue, (1, 30))],
        ),
        (
            "let x = if c { loop { if c { break }; break None } } else { o }",
            &[(Code::NoValue, (1, 30))],
        ),
        (
            "let x = loop { if c { break }; break if d { None } else { Some(print(\"a\")) } }",
            &[(Code::NoValue, (1, 23))],
        ),
        (
            "let x: string = loop { if c { break }; if d { break a }; break a }",
            &[(Code::NoValue, (1, 31))],
        ),
        (
            "let x = loop { if c { break }; break if d { 1 } else { \"s\" } }",
            &[(Code::NoValue, (1, 23))],
        ),
    ]);
}
