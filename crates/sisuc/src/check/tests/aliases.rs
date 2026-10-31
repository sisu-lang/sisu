//! Type aliases: an alias is the type that it names, its cycles and limits, and where a name in a
//! value position stands for it.

use std::fmt::Write as _;

use super::*;
use crate::check::alias;
use crate::types;

/// A class of two fields over two type parameters, before the aliases a row tests.
const PAIR: &str = "class Pair<A, B> {\n    first: A\n    second: B\n}\n";

/// The chain `type D1<T> = Pair<T, T>` to `type Dn<U> = Pair<D(n-1)<U>, D(n-1)<U>>`: each alias
/// names a parameter of its own, and its tree has 2^n leaves, though it has n distinct types.
fn doubling(n: usize) -> String {
    let mut src = "type D1<T> = Pair<T, T>\n".to_string();
    for k in 2..=n {
        writeln!(
            src,
            "type D{k}<P{k}> = Pair<D{}<P{k}>, D{}<P{k}>>",
            k - 1,
            k - 1
        )
        .expect("writing to a string should not fail");
    }
    src
}

/// The chain `type D1<T> = [T]` to `type Dn<T> = D(n-1)<D(n-1)<T>>`: each alias doubles the depth.
fn deepening(n: usize) -> String {
    let mut src = "type D1<T> = [T]\n".to_string();
    for k in 2..=n {
        writeln!(src, "type D{k}<T> = D{}<D{}<T>>", k - 1, k - 1)
            .expect("writing to a string should not fail");
    }
    src
}

/// The chain `type B1<T> = Pair<[T], T?>` to `type Bn<T> = Pair<B(n-1)<[T]>, B(n-1)<T?>>`: its
/// depth is 2n, but `Bn` has 2^n distinct types (`[T?]` is not `[T]?`).
fn branching(n: usize) -> String {
    let mut src = "type B1<T> = Pair<[T], T?>\n".to_string();
    for k in 2..=n {
        writeln!(src, "type B{k}<T> = Pair<B{}<[T]>, B{}<T?>>", k - 1, k - 1)
            .expect("writing to a string should not fail");
    }
    src
}

#[test]
fn aliases_should_check_clean_where_they_name_a_type() {
    for (row, src) in [
        (
            "an alias passed where its type is wanted, and back",
            "type Grid = [[char?]]\nclass Board {\n    cells: Grid\n}\nfn width(g: Grid) -> i64 { g.len() }\nfn same(g: [[char?]]) -> Grid { g }\nfn pass(g: [[char?]]) -> i64 { width(g) }\nfn elements(gs: [Grid]) -> [[char?]] { gs[0] }\nfn make(rows: [[char?]]) -> Board { Board(cells: rows) }\nfn main() {}",
        ),
        (
            "an alias inside a type argument",
            "type Grid = [[char?]]\nclass Stack<T> {\n    items: [T]\n}\nfn take(s: Stack<[[char?]]>) -> i64 { s.items.len() }\nfn give(s: Stack<Grid>) -> i64 { take(s) }\nfn back(s: Stack<[[char?]]>) -> Stack<Grid> { s }\nfn main() {}",
        ),
        (
            "an alias with a parameter",
            "type Pairs<T> = [[T]]\nfn f(p: Pairs<i64>) -> [[i64]] { p }\nfn g(p: [[i64]]) -> Pairs<i64> { p }\nfn h<U>(p: Pairs<U>) -> [[U]] { p }\nfn main() {}",
        ),
        (
            "an alias used before its declaration",
            "fn f(g: Grid) -> [i64] { g }\ntype Grid = Rows\ntype Rows = [i64]\nfn main() {}",
        ),
        (
            "an alias of a class, whose methods the alias extends",
            "class Point {\n    x: f64\n    y: f64\n}\ntype P = Point\nextend P {\n    fn norm(self) -> f64 { self.x + self.y }\n}\nfn main() {\n    let p = Point(x: 1.0, y: 2.0)\n    print(p.norm())\n}",
        ),
        (
            "a class that holds the alias that names it in an array",
            "type Grid = [Node]\nclass Node {\n    next: Grid?\n}\nfn main() {}",
        ),
        (
            "a parameter that the body does not use",
            "type P<T> = i64\nfn f(x: P<string>) -> i64 { x }\nfn main() {}",
        ),
        (
            "an alias that names an alias with the parameters it takes",
            "type Inner<T> = [T]\ntype Outer<T> = Inner<Inner<T>>\nfn f(x: Outer<i64>) -> [[i64]] { x }\nfn main() {}",
        ),
        (
            "a parameter that has the name of an alias",
            "type A = B<i64>\ntype B<A> = [A]\nfn f(x: A) -> [i64] { x }\nfn main() {}",
        ),
        (
            "a reversed chain, which resolves with no recursion",
            &reversed_chain(2_000),
        ),
        (
            "the patterns of an alias of an enum and of an optional type",
            "type O = i64?\nenum E {\n    Num(i64)\n    Nil\n}\ntype EE = E\nfn f(o: O, e: EE) -> i64 {\n    let a = match o {\n        O.Some(n) => n\n        O.None => 0\n    }\n    let b = match e {\n        EE.Num(n) => n\n        EE.Nil => 1\n    }\n    a + b\n}\nfn main() {\n    print(f(O.Some(1), EE.Nil))\n    print(f(O.None, EE.Num(1)))\n}",
        ),
        (
            "an optional alias with no type argument written, whose use fixes it",
            "type M<T> = T?\nfn main() {\n    let c: i64? = M.None\n    let d: i64? = M.Some(1)\n    print(c == None)\n    print(d == None)\n}",
        ),
        (
            "the paths of Option, through an alias of an optional type",
            "type O = i64?\ntype M<T> = T?\nfn show(x: i64?) { print(x == None) }\nfn main() {\n    let a: O = O.Some(1)\n    let b: O = O.None\n    let c = M<i64>.Some(1)\n    let d: string? = M<string>.None\n    show(a)\n    show(b)\n    show(c)\n    print(d == None)\n}",
        ),
    ] {
        clean_row(row, src);
    }
}

/// `type A1 = A2` to `type A<n> = i64`, used once: the first alias is resolved first, and the
/// chain below it is resolved from it.
fn reversed_chain(n: usize) -> String {
    let mut src = String::new();
    for k in 1..n {
        writeln!(src, "type A{k} = A{}", k + 1).expect("writing to a string should not fail");
    }
    format!("{src}type A{n} = i64\nfn f(x: A1) -> i64 {{ x }}\nfn main() {{}}")
}

/// The walk over the aliases keeps its path in a list, also for a cycle that holds many of them:
/// the cycle is one report, and the aliases on it are resolved with no recursion.
#[test]
fn a_cycle_of_many_aliases_should_report_once_without_recursion() {
    // 10x the smallest cycle whose recursion overflows the stack of a test thread.
    let n = 10_000;
    let mut src = String::new();
    for k in 1..n {
        writeln!(src, "type A{k} = A{}", k + 1).expect("writing to a string should not fail");
    }
    writeln!(src, "type A{n} = [A1]\nfn f(x: A1) {{}}\nfn main() {{}}")
        .expect("writing to a string should not fail");
    let (program, ds) = checked(&src);
    assert!(program.is_none());
    let [d] = &ds[..] else {
        panic!("{} diagnostics", ds.len());
    };
    assert_eq!((d.code(), at(&src, d.span())), (Code::AliasCycle, (1, 6)));
    // One secondary label for each step of the cycle.
    assert_eq!(d.secondary().len(), n);
}

/// `type A0 = Pair<A1, A1>`, `type Ak = Pair<A(k+1), A0>` and `type An = A0`: every alias writes
/// `A0`, so the component holds n cycles through `A0` that share their aliases, and the shortest
/// is `A0` and `A1`. With `own`, `A0` also writes itself, as `Pair<A1, A0>`.
fn overlapping(n: usize, own: bool) -> String {
    let mut src = format!(
        "class Pair<X, Y> {{}}\ntype A0 = Pair<A1, {}>\n",
        if own { "A0" } else { "A1" }
    );
    for k in 1..n {
        writeln!(src, "type A{k} = Pair<A{}, A0>", k + 1)
            .expect("writing to a string should not fail");
    }
    format!("{src}type A{n} = A0\nfn main() {{}}")
}

/// `type A0 = Pair<A1, A2>`, `type A1 = A0` and a long ring `A2` to `An` back to `A0`: the first
/// cycle that a walk finds by going deep is the long one, and the shortest is `A0` and `A1`.
fn long_first(n: usize) -> String {
    let mut src = "class Pair<X, Y> {}\ntype A0 = Pair<A1, A2>\ntype A1 = A0\n".to_string();
    for k in 2..n {
        writeln!(src, "type A{k} = Pair<A{}, A{}>", k + 1, k + 1)
            .expect("writing to a string should not fail");
    }
    format!("{src}type A{n} = A0\nfn main() {{}}")
}

/// A component of overlapping cycles is one report, at the first alias, with a shortest cycle
/// through it: the report does not name the whole component.
#[test]
fn overlapping_alias_cycles_should_report_once_at_a_shortest_cycle() {
    for (row, src, labels) in [
        (
            "a cycle of two aliases is the shortest",
            overlapping(50, false),
            2,
        ),
        (
            "an alias that writes itself is the shortest",
            overlapping(50, true),
            1,
        ),
        (
            "a short cycle beside a long one that a deep walk finds first",
            long_first(50),
            2,
        ),
    ] {
        let (_, ds) = checked(&src);
        let [d] = &ds[..] else {
            panic!("{row}: {} diagnostics", ds.len());
        };
        assert_eq!(
            (d.code(), at(&src, d.span())),
            (Code::AliasCycle, (2, 6)),
            "{row}"
        );
        assert_eq!(d.secondary().len(), labels, "{row}");
    }
}

/// Finding the cycle of a component reads each edge once, however many cycles overlap: the steps
/// grow with the aliases, not with their square. 100 aliases is 10x the smallest size at which a
/// walk that follows each back edge shows a quadratic count.
#[test]
fn the_search_for_an_alias_cycle_should_read_each_edge_once() {
    let steps = |n| {
        crate::graph::CYCLE_STEPS.set(0);
        let (_, ds) = checked(&overlapping(n, false));
        assert_eq!(ds.len(), 1);
        crate::graph::CYCLE_STEPS.get()
    };
    assert_near_linear(steps(100), steps(200), steps(300));
}

/// A depth-first resolution that follows each alias by a call would use a frame for each alias of
/// the chain: 100,000 aliases overflow the stack. The alias that waits for its body takes a place
/// in a list, so the resolution of a chain has no recursion.
#[test]
fn a_chain_of_aliases_should_resolve_without_recursion() {
    clean(&reversed_chain(100_000));
}

/// An alias that refers to itself, directly or through others, is one `AliasCycle` at the first
/// alias of the cycle in source order, with a secondary label at each reference that closes a
/// step.
#[test]
fn an_alias_cycle_should_be_reported_once_at_the_first_alias() {
    for (row, src, at, secondary) in [
        (
            "directly",
            "type A = [A]\nfn main() {}",
            (1, 6),
            vec![(1, 11)],
        ),
        (
            "through an optional",
            "type A = A?\nfn main() {}",
            (1, 6),
            vec![(1, 10)],
        ),
        (
            "through another alias",
            "type A = B\ntype B = [A]\nfn main() {}",
            (1, 6),
            vec![(1, 10), (2, 11)],
        ),
        (
            "through the argument of an alias",
            "type P<T> = [T]\ntype Q = P<Q>\nfn main() {}",
            (2, 6),
            vec![(2, 12)],
        ),
        (
            "from a later alias that the first uses",
            "type B = A\ntype A = [B]\nfn main() {}",
            (1, 6),
            vec![(1, 10), (2, 11)],
        ),
        (
            "twice in one body, which closes the same cycle again",
            "class Pair<A, B> {\n    a: A\n    b: B\n}\ntype A = Pair<A, A>\nfn main() {}",
            (5, 6),
            vec![(5, 15)],
        ),
        (
            "through three aliases",
            "type A = B\ntype B = C\ntype C = [A]\nfn main() {}",
            (1, 6),
            vec![(1, 10), (2, 10), (3, 11)],
        ),
    ] {
        // The alias is named by one letter, which follows `type ` on its line.
        let name = &src.lines().nth(at.0 - 1).unwrap_or_default()[5..6];
        diagnostic_row(row, src, Code::AliasCycle, (at.0, at.1, name), &secondary);
    }
}

/// A cycle gives `Error` to each alias on it and to each alias that uses one, so a use reports
/// nothing more, however many there are.
#[test]
fn a_cyclic_alias_should_report_nothing_for_its_uses() {
    for (row, src) in [
        (
            "uses in three signatures",
            "type A = [A]\nfn f(x: A) {}\nfn g(x: A) -> A { x }\nclass C {\n    a: A\n}\nfn main() {}",
        ),
        (
            "an alias that uses the cycle",
            "type A = [A]\ntype B = [A]\nfn f(x: B) {}\nfn main() {}",
        ),
        (
            "an alias of the alias that is used first",
            "fn f(x: B) {}\ntype B = A\ntype A = [B]\nfn main() {}",
        ),
    ] {
        let found = found(src);
        assert_eq!(found.len(), 1, "{row}: {found:?}");
        assert_eq!(found[0].0, Code::AliasCycle, "{row}");
    }
}

/// The walk enters each alias once, also an alias that names itself, and never follows it again:
/// without the path, `type A = [A]` would not end.
#[test]
fn an_alias_that_names_itself_should_be_entered_once() {
    alias::VISITS.set(0);
    alias::VISIT_BUDGET.set(1_000);
    diagnostic(
        "type A = [A]\nfn main() {}",
        Code::AliasCycle,
        (1, 6, "A"),
        &[(1, 11)],
    );
    assert_eq!(alias::VISITS.get(), 1);
    // Two aliases on one cycle, and a third that no cycle holds.
    alias::VISITS.set(0);
    let ds = diags("type A = [B]\ntype B = [A]\ntype C = i64\nfn main() {}");
    assert_eq!(ds.len(), 1);
    assert_eq!(alias::VISITS.get(), 3);
}

/// A class or an enum that holds an alias that names it breaks no cycle: it is a name, not an
/// expansion.
#[test]
fn an_alias_should_report_what_a_signature_names_wrongly() {
    for (row, src, expected) in [
        (
            "a class and an alias of one name, alias first",
            "type Grid = [i64]\nclass Grid {}\nfn main() {}",
            vec![(Code::DefinedTwice, (2, 7))],
        ),
        (
            "a class and an alias of one name, class first",
            "class Grid {}\ntype Grid = [i64]\nfn main() {}",
            vec![(Code::DefinedTwice, (2, 6))],
        ),
        (
            "an alias that takes the name of a built-in type",
            "type i64 = bool\nfn main() {}",
            vec![(Code::BuiltInName, (1, 6))],
        ),
        (
            "an alias used without its arguments",
            "type Pairs<T> = [[T]]\nfn f(p: Pairs) {}\nfn main() {}",
            vec![(Code::TypeArguments, (2, 9))],
        ),
        (
            "an alias that takes no arguments, used with one",
            "type Grid = [i64]\nfn f(g: Grid<i64>) {}\nfn main() {}",
            vec![(Code::TypeArguments, (2, 13))],
        ),
        (
            "an alias of an interface",
            "interface Shape {\n    fn area(self) -> f64\n}\ntype S = Shape\nfn main() {}",
            vec![(Code::InterfaceAsType, (4, 10))],
        ),
        (
            "an extend block of an alias of a built-in type",
            "type Grid = [[char?]]\nextend Grid {}\nfn main() {}",
            vec![(Code::CannotExtend, (2, 8))],
        ),
    ] {
        codes_row(row, src, &expected);
    }
}

/// The alias that makes a type pass 256 levels is reported at the first written type, in source
/// order, whose type passes it: an alias does not lower the depth.
#[test]
fn an_alias_that_nests_too_deep_should_report_the_written_type() {
    for (row, src, at, written) in [
        (
            "an alias that doubles the depth, used where it is made",
            format!("{}fn f(x: D40<i64>) {{}}\nfn main() {{}}", deepening(40)),
            (10, 15),
            "D9<D9<T>>",
        ),
        (
            "an array around the deepest alias",
            format!(
                "type A1 = [i64]\n{}fn main() {{}}",
                (2..=257).fold(String::new(), |mut src, k| {
                    writeln!(src, "type A{k} = [A{}]", k - 1).expect("writing should not fail");
                    src
                })
            ),
            (257, 13),
            "[A256]",
        ),
        (
            "an alias that names the deepest one with its parameter",
            format!(
                "{}type Inner<T> = D9<T>\ntype D10<T> = D9<Inner<T>>\nfn main() {{}}",
                deepening(9)
            ),
            (11, 15),
            "D9<Inner<T>>",
        ),
    ] {
        types::start_substituting(10_000);
        diagnostic_row(row, &src, Code::NestingTooDeep, (at.0, at.1, written), &[]);
    }
}

/// The work of one written type is bounded by the types it adds to the memo, whatever the types
/// that the process interned before: the same program, checked twice, gives the same report.
#[test]
fn an_alias_that_builds_too_many_types_should_report_the_written_type() {
    let src = format!("{PAIR}{}fn main() {{}}", branching(30));
    for _ in 0..2 {
        types::start_substituting(1_000_000);
        let d = diagnostic(
            &src,
            Code::AliasTooLarge,
            (20, 15, "Pair<B15<[T]>, B15<T?>>"),
            &[],
        );
        assert_eq!(d.code().as_str(), "E0162");
    }
}

/// A bound that names a parameter of an alias is checked at each use, on the type the use names.
#[test]
fn a_bound_in_an_alias_body_should_be_checked_at_each_use() {
    const HEAP: &str = "class Heap<T: Ord> {\n    items: [T]\n}\n";
    let pair = format!("{PAIR}{HEAP}");
    for (row, src, expected) in [
        (
            "an alias of a bounded class, used with a type that meets it",
            format!("{HEAP}type H<T> = Heap<T>\nfn f(h: H<i64>) {{}}\nfn main() {{}}"),
            vec![],
        ),
        (
            "an alias of a bounded class, used with a type that fails it",
            format!("{HEAP}type H<T> = Heap<T>\nfn f(h: H<bool>) {{}}\nfn main() {{}}"),
            vec![(Code::NotConform, (5, 9))],
        ),
        (
            "an alias that names an alias",
            format!(
                "{HEAP}type H<T> = Heap<T>\ntype H2<T> = H<T>\nfn f(a: H2<i64>, b: H2<bool>) {{}}\nfn main() {{}}"
            ),
            vec![(Code::NotConform, (6, 21))],
        ),
        (
            "a bounded class twice in an alias body",
            format!(
                "{pair}type HH<T> = Pair<Heap<T>, Heap<T>>\nfn f(h: HH<bool>) {{}}\nfn main() {{}}"
            ),
            vec![(Code::NotConform, (9, 9))],
        ),
        (
            "a bounded class with no parameter, in the body",
            format!("{HEAP}type H0 = Heap<bool>\nfn f(a: H0, b: H0) {{}}\nfn main() {{}}"),
            vec![(Code::NotConform, (4, 16))],
        ),
        (
            "an alias of a bounded enum, used with a type that fails it",
            "enum Heap2<T: Ord> {\n    One(T)\n    Zero\n}\ntype H<T> = Heap2<T>\nfn f(h: H<bool>) {}\nfn main() {}".to_string(),
            vec![(Code::NotConform, (6, 9))],
        ),
        (
            "a bounded class with a parameter inside an array",
            format!("{HEAP}type HA<T> = [Heap<[T]>]\nfn f(a: HA<i64>) {{}}\nfn main() {{}}"),
            vec![(Code::NotConform, (5, 9))],
        ),
    ] {
        codes_row(row, &src, &expected);
    }
}

/// `T.Item` in an alias body names the one interface that declares `Item`, and each use checks
/// that its argument conforms to it, then gives the type that the conformance chose.
#[test]
fn an_associated_type_of_an_alias_parameter_should_name_the_one_interface_that_declares_it() {
    const ITER: &str = "interface Iter {\n    type Item\n    fn first(self) -> Item\n}\nclass Ints: Iter {\n    type Item = i64\n    n: i64\n    fn first(self) -> Item { self.n }\n}\n";
    const OTHER: &str = "interface Keyed {\n    type Item\n}\n";
    for (row, src, expected) in [
        (
            "a use whose argument conforms, in a signature",
            format!("{ITER}type X<T> = T.Item\nfn f(x: X<Ints>) -> i64 {{ x }}\nfn main() {{}}"),
            vec![],
        ),
        (
            "a use whose argument conforms, in a body",
            format!(
                "{ITER}type X<T> = T.Item\nfn main() {{\n    let x: X<Ints> = 1\n    print(x)\n}}"
            ),
            vec![],
        ),
        (
            "a use through a type parameter that has the bound",
            format!(
                "{ITER}type X<T> = T.Item\nfn f<S: Iter>(x: X<S>) -> S.Item {{ x }}\nfn main() {{}}"
            ),
            vec![],
        ),
        (
            "a use whose argument does not conform",
            format!("{ITER}type X<T> = T.Item\nfn f(x: X<string>) {{}}\nfn main() {{}}"),
            vec![(Code::NotConform, (11, 9))],
        ),
        (
            "a use in a body whose argument does not conform",
            format!(
                "{ITER}type X<T> = T.Item\nfn main() {{\n    let x: X<string> = 1\n    print(x)\n}}"
            ),
            vec![(Code::NotConform, (12, 12))],
        ),
        (
            "an associated type that no interface declares",
            format!("{ITER}type Y<T> = T.Itme\nfn main() {{}}"), // spellchecker:disable-line
            vec![(Code::UnknownType, (10, 15))],
        ),
        (
            "an associated type that two interfaces declare",
            format!("{ITER}{OTHER}type Z<T> = T.Item\nfn main() {{}}"),
            vec![(Code::Ambiguous, (13, 15))],
        ),
        (
            "an associated type of an associated type",
            format!("{ITER}type Y<T> = T.Item.Item\nfn main() {{}}"),
            vec![(Code::UnknownType, (10, 13))],
        ),
        (
            "an associated type that a generic interface declares",
            "interface Gen<K> {\n    type Item\n}\ntype G<T> = T.Item\nfn main() {}".to_string(),
            vec![(Code::UnknownType, (4, 15))],
        ),
    ] {
        codes_row(row, &src, &expected);
    }
}

/// A name in a value position stands for the type that the alias names: a constructor builds the
/// class, a path names its variants, a number type converts and has its members, and each reports
/// as the type it names does.
#[test]
fn an_alias_should_stand_for_its_type_in_every_value_position() {
    const POINT: &str = "class Point {\n    x: f64\n    y: f64\n}\n";
    const EXPR: &str = "enum Expr {\n    Num(i64)\n    Neg(Expr)\n    Nil\n}\n";
    const BOX: &str = "class Box<T> {\n    value: T\n}\n";
    const HEAP: &str = "class Heap<T: Ord> {\n    item: T\n}\n";
    with_prelude();
    for (row, src) in [
        (
            "a constructor of an alias of a class",
            format!("{POINT}type P = Point\nfn main() {{\n    let p: Point = P(x: 1.0, y: 2.0)\n    print(p.x)\n}}"),
        ),
        (
            "a constructor of an alias of an alias of a class",
            format!("{POINT}type P = Point\ntype P2 = P\nfn main() {{\n    let p: Point = P2(x: 1.0, y: 2.0)\n    print(p.x)\n}}"),
        ),
        (
            "a variant of an alias of an enum",
            format!("{EXPR}type E = Expr\nfn main() {{\n    let e: Expr = E.Num(1)\n    let n: Expr = E.Neg(e)\n    let z: Expr = E.Nil\n    print(\"{{n == z}}\")\n}}"),
        ),
        (
            "a pattern of an alias of an enum",
            format!("{EXPR}type E = Expr\nfn f(e: Expr) -> i64 {{\n    match e {{\n        E.Num(n) => n\n        E.Neg(_) => 0\n        E.Nil => 1\n    }}\n}}\nfn g(e: Expr) -> i64 {{\n    if let E.Num(n) = e {{\n        return n\n    }}\n    0\n}}\nfn h(e: Expr) -> i64 {{\n    let mut n = 0\n    let mut cur = e\n    while let E.Neg(inner) = cur {{\n        n += 1\n        cur = inner\n    }}\n    n\n}}\nfn main() {{}}"),
        ),
        (
            "an alias of a generic class, its parameters inferred",
            format!("{BOX}type B2<T> = Box<T>\nfn main() {{\n    let b: Box<i64> = B2(value: 1)\n    let c: Box<i64> = B2<i64>(value: 1)\n    print(b.value + c.value)\n}}"),
        ),
        (
            "an alias of a generic enum, whose variant waits for the context of a call",
            "type R2<T, E> = Result<T, E>\nfn id<T>(x: T) -> T { x }\nfn main() {\n    let r: Result<i64, string> = id(R2.Ok(1))\n    print(\"{r == r}\")\n}".to_string(),
        ),
        (
            "an alias of a generic class, whose constructor waits for the context of a call",
            "class Holder<T> {\n    items: [T]\n}\ntype HA<T> = Holder<T>\nfn id<T>(x: T) -> T { x }\nfn main() {\n    let h: Holder<i64> = id(HA(items: []))\n    print(h.items.len())\n}".to_string(),
        ),
        (
            "an alias of a generic class with a fixed argument",
            format!("{BOX}type B = Box<i64>\nfn main() {{\n    let b: Box<i64> = B(value: 1)\n    print(b.value)\n}}"),
        ),
        (
            "an alias that changes the order of the arguments",
            "class Pair<A, B> {\n    a: A\n    b: B\n}\ntype Swap<A, B> = Pair<B, A>\nfn main() {\n    let p: Pair<string, i64> = Swap<i64, string>(a: \"x\", b: 1)\n    let q: Pair<string, i64> = Swap(a: \"y\", b: 2)\n    print(p.a + q.a)\n}".to_string(),
        ),
        (
            "variants of an alias of a generic enum",
            "type R<T> = Result<T, string>\ntype RI = Result<i64, string>\nfn main() {\n    let a: Result<i64, string> = R.Ok(1)\n    let b: Result<i64, string> = RI.Err(\"e\")\n    let c: Result<i64, string> = R<i64>.Ok(2)\n    print(\"{a == b}\")\n}".to_string(),
        ),
        (
            "the variants of an alias of an optional",
            "type O = i64?\nfn main() {\n    let a: i64? = O.None\n    let b: i64? = O.Some(1)\n    print(\"{a == b}\")\n}".to_string(),
        ),
        (
            "an alias of a bounded class that meets its bound",
            format!("{HEAP}type H<T> = Heap<T>\ntype H2<T> = H<T>\nfn f(a: H<i64>, b: H2<i64>) -> i64 {{ a.item + b.item }}\nfn main() {{\n    let h: Heap<i64> = H(item: 1)\n    print(f(h, h))\n}}"),
        ),
        (
            "an alias of a number type, its conversion and its constants",
            "type F = f64\nconst X: F = F(1)\nconst Y: F = F.pi\nfn main() {\n    let a: F = F(3)\n    let b: f64 = F.pi\n    print(a + b + X + Y)\n}".to_string(),
        ),
        (
            "an alias of a type through a parameter that no part names",
            format!("{BOX}type C<T> = Box<i64>\nfn main() {{\n    let c: Box<i64> = C<string>(value: 1)\n    print(c.value)\n}}"),
        ),
    ] {
        clean_row(row, &src);
    }
}

/// The doubling chain `D1` to `D40`, used 20 times as `D40<i64>` and once at each level, takes
/// 41 substitutions, not 860: every alias shares one memo, and a parameter is named by place.
#[test]
fn a_chain_of_aliases_should_share_the_work_of_its_substitutions() {
    let mut uses = String::new();
    for i in 0..20 {
        writeln!(uses, "fn f{i}(x: D40<i64>) {{}}").expect("writing to a string should not fail");
    }
    for k in 1..=40 {
        writeln!(uses, "fn g{k}(x: D{k}<i64>) {{}}").expect("writing to a string should not fail");
    }
    let src = format!("{PAIR}{}{uses}fn main() {{}}", doubling(40));
    types::start_substituting(400);
    clean(&src);
    assert!(
        types::substituted() <= 400,
        "{} substitutions",
        types::substituted()
    );
}

/// The wrong constructor of an alias reports what the class or the enum reports, with the
/// arguments that the alias fixes.
#[test]
fn an_alias_should_report_what_a_value_position_names_wrongly() {
    const BOX: &str = "class Box<T> {\n    value: T\n}\n";
    with_prelude();
    for (row, src, expected) in [
        (
            "an alias of a type with no constructor",
            "type G = [i64]\nfn main() {\n    let g = G(1)\n}".to_string(),
            vec![(Code::TypeAsValue, (3, 13))],
        ),
        (
            "an alias of an enum, which has no constructor",
            "enum Expr {\n    Nil\n}\ntype E = Expr\nfn main() {\n    let g = E(1)\n}".to_string(),
            vec![(Code::TypeAsValue, (6, 13))],
        ),
        (
            "an alias that fixes an argument, built with another",
            format!("{BOX}type B = Box<i64>\nfn main() {{\n    let b = B(value: \"x\")\n}}"),
            vec![(Code::Mismatch, (6, 22))],
        ),
        (
            "a variant of an alias that fixes an argument",
            "type R<T> = Result<T, string>\ntype RI = Result<i64, string>\nfn main() {\n    let a: R<i64> = R.Err(5)\n    let b: RI = RI.Err(5)\n}".to_string(),
            vec![(Code::Mismatch, (4, 27)), (Code::Mismatch, (5, 24))],
        ),
        (
            "a conversion to an alias of a number type that can fail",
            "type U = u8\nfn main() {\n    let n = 300\n    let x: U = U(n)\n    print(x)\n}".to_string(),
            vec![(Code::Mismatch, (4, 16))],
        ),
        (
            "a field that the alias names by the class's parameter",
            "class Box<T> {\n    value: [T]\n}\ntype B2<T> = Box<T>\nfn main() {\n    let b = B2(value: \"x\")\n}".to_string(),
            vec![(Code::Mismatch, (6, 23))],
        ),
        (
            "a bound that the type an alias builds fails",
            "class Heap<T: Ord> {\n    item: T\n}\ntype H<T> = Heap<T>\nfn main() {\n    let h = H(item: true)\n}".to_string(),
            vec![(Code::NotConform, (6, 13))],
        ),
    ] {
        codes_row(row, &src, &expected);
    }
}

/// The source of the diagnostic fixture `name`, which also pins the wording of its reports.
macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("../../../tests/diagnostics/", $name, ".sisu"))
    };
}

/// A use of an alias in a value position makes what the alias's own type makes of it: the type
/// arguments it writes are resolved, counted and checked, those it leaves out are bound by the
/// value, and the bounds that the alias's body puts on its parameters hold for what they are bound
/// to.
#[test]
fn a_value_position_should_check_the_type_that_its_alias_names() {
    for (row, src, expected) in [
        (
            "bounds nested in the body, in a constructor, a variant and an optional",
            fixture!("E0154-alias-nested"),
            vec![
                (Code::NotConform, (24, 13)),
                (Code::NotConform, (25, 13)),
                (Code::NotConform, (26, 32)),
                (Code::NotConform, (27, 13)),
                (Code::NotConform, (28, 13)),
                (Code::NotConform, (29, 34)),
                (Code::NotConform, (30, 13)),
            ],
        ),
        (
            "an annotation that failed its bound is not reported again at the variant",
            fixture!("E0154-alias-enum"),
            vec![(Code::NotConform, (9, 12)), (Code::NotConform, (10, 13))],
        ),
        (
            "an optional alias binds its parameters from the shape of its body",
            fixture!("E0032-alias-optional-shape"),
            vec![(Code::Mismatch, (4, 26)), (Code::Mismatch, (5, 20))],
        ),
        (
            "an unknown type argument, in a conversion, a qualifier and a constructor",
            fixture!("E0030-alias-argument"),
            vec![
                (Code::UnknownType, (5, 13)),
                (Code::UnknownType, (6, 13)),
                (Code::UnknownType, (7, 17)),
            ],
        ),
        (
            "too many type arguments",
            fixture!("E0137-alias-arity"),
            vec![
                (Code::TypeArguments, (4, 12)),
                (Code::TypeArguments, (5, 12)),
            ],
        ),
        (
            "type arguments for an alias that takes none, which an alias of an alias is",
            fixture!("E0137-alias-conversion"),
            vec![
                (Code::TypeArguments, (5, 12)),
                (Code::TypeArguments, (6, 12)),
            ],
        ),
        (
            "a pattern qualifier holds the arguments that the alias fixes",
            fixture!("E0032-alias-pattern"),
            vec![
                (Code::Mismatch, (13, 9)),
                (Code::Mismatch, (14, 9)),
                (Code::Mismatch, (20, 9)),
                (Code::Mismatch, (21, 9)),
                (Code::Mismatch, (27, 9)),
                (Code::Mismatch, (28, 9)),
                (Code::Mismatch, (34, 9)),
                (Code::Mismatch, (35, 9)),
            ],
        ),
        (
            "a local cannot take the name of an alias of an enum",
            fixture!("E0116-alias"),
            vec![(Code::EnumNamedLocal, (9, 9))],
        ),
    ] {
        codes_row(row, src, &expected);
    }
}

/// An alias of an optional type has an empty optional that every comparison with `None` takes as
/// `None` itself, and an alias of a built-in type is a built-in type in a qualifier and in a
/// constant.
#[test]
fn an_alias_should_stand_for_its_type_where_a_comparison_or_a_constant_reads_it() {
    const BOX: &str = "class Box<U> {\n    value: U?\n}\n";
    with_prelude();
    for (row, src) in [
        (
            "an empty optional through an alias, on either side of ==, and in an array",
            format!("{BOX}type OP = Box<i64>?\ntype M<T> = Box<T>?\nfn main() {{\n    let p: Box<i64>? = None\n    print(p == OP.None)\n    print(OP.None != p)\n    print(p == M<i64>.None)\n    print(p == M.None)\n    let xs = [M.None, p]\n    print(xs.len())\n}}"),
        ),
        (
            "constants and qualifiers through an alias with a parameter",
            "type F<T> = f64\ntype G = F<i64>\nconst X: f64 = F<i64>(3)\nconst Y: f64 = F<i64>.pi\nconst Z: f64 = G(3)\nfn main() {\n    print(F<string>.pi + F.pi + G.pi + X + Y + Z)\n}".to_string(),
        ),
    ] {
        clean_row(row, &src);
    }
}
