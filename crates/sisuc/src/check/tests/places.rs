//! Element places: their lowering, the temps of compound operators, the spill, and which roots
//! may be written.

use super::*;

/// `class C` with a `mut` array field and a method that writes into it.
const C: &str =
    "class C {\n    mut cells: [i64]\n    fn set(self, i: i64) { self.cells[i] = 0 }\n}\n";

#[test]
fn lowers_element_writes() {
    let rows = [
        (
            "fn f() {\n    let mut xs = [0]\n    xs[0] = 9\n}\nfn main() {}".to_owned(),
            "(= (index xs#0 0) 9)",
        ),
        (
            "fn f(y: i64, x: i64) {\n    let mut grid = [[false]]\n    grid[y][x] = true\n}\nfn main() {}"
                .to_owned(),
            "(= (index (index grid#2 y#0) x#1) true)",
        ),
        (
            format!("{C}fn main() {{}}"),
            "(= (index (. self#0 cells) i#1) 0)",
        ),
    ];
    for (src, part) in rows {
        contains(&src, &[part]);
    }
}

/// Each index, and a field root's base, is bound once, unless it is an immutable local; the root
/// binding is not.
#[test]
fn lowers_compound_element_writes() {
    let rows = [
        (
            "fn f(i: i64) {\n    let mut xs = [0]\n    xs[i] += 1\n}\nfn main() {}".to_owned(),
            "(block (= (index xs#1 i#0) (+ (index xs#1 i#0) 1)))",
        ),
        (
            "fn f(i: i64) {\n    let mut j = i\n    let mut xs = [0]\n    xs[j] += 1\n    j += 1\n}\nfn main() {}"
                .to_owned(),
            "(block (let #3 j#1) (= (index xs#2 #3) (+ (index xs#2 #3) 1)))",
        ),
        (
            format!("{C}fn g() {{\n    let c = C(cells: [0])\n    c.cells[0] += 1\n}}\nfn main() {{}}"),
            "(block (let #1 0) (= (index (. c#0 cells) #1) (+ (index (. c#0 cells) #1) 1)))",
        ),
        (
            "fn n() -> i64 { 0 }\nfn g() {\n    let mut grid = [[0]]\n    grid[n()][n()] += 1\n}\nfn main() {}"
                .to_owned(),
            "(block (let #1 (call n)) (let #2 (call n)) (= (index (index grid#0 #1) #2) (+ (index (index grid#0 #1) #2) 1)))",
        ),
        (
            format!(
                "{C}fn make() -> C {{ C(cells: [0]) }}\nfn g() {{\n    make().cells[0] += 1\n}}\nfn main() {{}}"
            ),
            "(block (let #0 (call make)) (let #1 0) (= (index (. #0 cells) #1) (+ (index (. #0 cells) #1) 1)))",
        ),
    ];
    for (src, part) in rows {
        contains(&src, &[part]);
    }
}

/// Distinct calls, so an index bound out of source order changes the lowering.
#[test]
fn binds_indexes_in_source_order() {
    let printed = lowered(
        "fn outer() -> i64 { 0 }\nfn inner() -> i64 { 0 }\nfn g() {\n    let mut grid = [[0]]\n    grid[outer()][inner()] += 1\n}\nfn main() {}",
    );
    assert!(
        printed.contains(
            "(block (let #1 (call outer)) (let #2 (call inner)) (= (index (index grid#0 #1) #2) (+ (index (index grid#0 #1) #2) 1)))"
        ),
        "{printed}"
    );
    for call in ["(call outer)", "(call inner)"] {
        assert_eq!(printed.matches(call).count(), 1, "{call} in {printed}");
    }
}

/// `a` is counted and the value can exit, so the place's operands are bound before it runs.
#[test]
fn spills_an_element_write() {
    contains(
        "class A {\n    mut f: [i64]\n}\nfn g(a: A, i: i64, c: bool) -> i64 {\n    a.f[i] = if c { return 0 } else { 1 }\n    0\n}\nfn main() {}",
        &[
            "(block (let #3 a#0) (let #4 i#1) (let #5 (if c#2 (block (return 0)) (block 1))) (= (index (. #3 f) #4) #5))",
        ],
    );
}

/// An index that can exit makes the element write exit, so the call's counted `make()` spills.
#[test]
fn an_exiting_index_spills_the_enclosing_call() {
    assert_eq!(
        printed_g(
            "fn g(c: bool) -> i64 {\n    let mut xs = [0]\n    f(make(), if true {\n        xs[if c { return 0 } else { 0 }] = 1\n        0\n    } else { 0 })\n    2\n}"
        ),
        "(fn g ((c#0 bool)) i64 (block (let mut xs#1 (array 0)) (block (let #2 (call make)) (let #3 (if true (block (= (index xs#1 (if c#0 (block (return 0)) (block 0))) 1) 0) (block 0))) (call f #2 #3)) 2))"
    );
}

/// `class P` with a `mut` string field.
const P: &str = "class P {\n    mut name: string\n}\n";

/// `+=` and `= +` on a field or an element whose `+` reads the same place append to it.
#[test]
fn recognises_appends() {
    let printed = lowered(&format!(
        "{P}fn k() -> i64 {{ 0 }}\nfn f(p: P, i: i64, t: string) {{\n    let mut names = [\"a\"]\n    p.name += \"x\"\n    p.name = p.name + \"x\"\n    names[i] += \"x\"\n    names[0] = names[0] + \"x\"\n    names[k()] += t\n}}\nfn main() {{}}"
    ));
    assert!(
        printed.contains(
            "(block (let mut names#3 (array \"a\")) (block (append (. p#0 name) \"x\")) (append (. p#0 name) \"x\") (block (append (index names#3 i#1) \"x\")) (append (index names#3 0) \"x\") (block (let #4 (call k)) (append (index names#3 #4) t#2)))"
        ),
        "{printed}"
    );
}

/// A `+` that reads another place, reads it through a different literal index or an index that
/// is not a local or a literal,
/// prepends, or has a right operand that can exit stays an assignment of a `concat`.
#[test]
fn does_not_recognise() {
    let printed = lowered(&format!(
        "{P}fn f() -> i64 {{ 0 }}\nfn g(p: P, q: P, c: bool) {{\n    let mut names = [\"a\"]\n    p.name = q.name + \"x\"\n    names[0] = names[1] + \"x\"\n    names[f()] = names[f()] + \"x\"\n    p.name = \"x\" + p.name\n    p.name += (if c {{ return }} else {{ \"x\" }})\n}}\nfn main() {{}}"
    ));
    assert!(!printed.contains("(append"), "{printed}");
    assert!(
        printed.contains(
            "(= (. p#0 name) (concat (. q#1 name) \"x\")) (= (index names#3 0) (concat (index names#3 1) \"x\")) (= (index names#3 (call f)) (concat (index names#3 (call f)) \"x\")) (= (. p#0 name) (concat \"x\" (. p#0 name))) (block (block (let #6 p#0) (let #7 (block (let #4 (. p#0 name)) (let #5 (if c#2 (block (return)) (block \"x\"))) (concat #4 #5))) (= (. #6 name) #7)))"
        ),
        "{printed}"
    );
}

/// "The same place" follows a chain of fields and an index inside a field root's base.
#[test]
fn same_place_through_fields_and_elements() {
    let printed = lowered(
        "class Q {\n    mut name: string\n}\nclass P {\n    q: Q\n    r: Q\n}\nfn f(p: P, ps: [P], i: i64, j: i64, q: Q) {\n    p.q.name = p.q.name + \"x\"\n    ps[i].q.name = ps[i].q.name + \"x\"\n    p.q.name = p.r.name + \"x\"\n    ps[i].q.name = ps[j].q.name + \"x\"\n    p.q.name = q.name + \"x\"\n}\nfn main() {}",
    );
    assert!(
        printed.contains(
            "(append (. (. p#0 q) name) \"x\") (append (. (. (index ps#1 i#2) q) name) \"x\") (= (. (. p#0 q) name) (concat (. (. p#0 r) name) \"x\")) (= (. (. (index ps#1 i#2) q) name) (concat (. (. (index ps#1 j#3) q) name) \"x\")) (= (. (. p#0 q) name) (concat (. q#4 name) \"x\"))"
        ),
        "{printed}"
    );
}

#[test]
fn element_type_is_expected_of_the_value() {
    contains(
        "fn f() {\n    let mut xs: [[i64]] = [[1]]\n    xs[0] = []\n}\nfn main() {}",
        &["(= (index xs#0 0) (array))"],
    );
    contains(
        "fn f(i: i64) {\n    let mut bytes: [u8] = [0]\n    bytes[i] += 1\n}\nfn main() {}",
        &["(+ (index bytes#1 i#0) 1u8)"],
    );
}

/// `let` does not reach into the class objects an array holds.
#[test]
fn field_of_an_element_of_a_let_array() {
    clean("class N {\n    mut v: i64\n}\nfn main() {\n    let a = [N(v: 1)]\n    a[0].v = 2\n}");
}

#[test]
fn element_of_a_field_of_a_call() {
    clean(&format!(
        "{C}fn make() -> C {{ C(cells: [0]) }}\nfn main() {{\n    make().cells[0] = 1\n}}"
    ));
}

#[test]
fn element_write_counts_as_reassigning() {
    clean("fn main() {\n    let mut xs = [0]\n    xs[0] = 1\n}");
}

#[test]
fn element_of_a_let() {
    let src = "fn main() {\n    let xs = [0]\n    xs[0] = 1\n}";
    let d = diagnostic(src, Code::AssignToImmutable, (3, 5, "xs[0] = 1"), &[(2, 9)]);
    fix_removes_its_code(src, &d);
}

#[test]
fn element_of_a_parameter() {
    diagnostic(
        "fn f(xs: [i64]) { xs[0] = 1 }\nfn main() {}",
        Code::AssignToImmutable,
        (1, 19, "xs[0] = 1"),
        &[(1, 6)],
    );
}

#[test]
fn element_of_an_if_let_binding() {
    diagnostic(
        "fn f(m: [i64]?) { if let xs = m { xs[0] = 1 } }\nfn main() {}",
        Code::AssignToImmutable,
        (1, 35, "xs[0] = 1"),
        &[(1, 26)],
    );
}

#[test]
fn element_of_a_for_variable() {
    diagnostic(
        "fn f(grid: [[i64]]) { for row in grid { row[0] = 1 } }\nfn main() {}",
        Code::AssignToImmutable,
        (1, 41, "row[0] = 1"),
        &[(1, 27)],
    );
}

#[test]
fn element_of_a_while_let_binding() {
    diagnostic(
        "fn f(m: [i64]?) { while let xs = m { xs[0] = 1 } }\nfn main() {}",
        Code::AssignToImmutable,
        (1, 38, "xs[0] = 1"),
        &[(1, 29)],
    );
}

#[test]
fn element_of_a_bare_field() {
    diagnostic(
        "class C {\n    cells: [i64]\n    fn set(self) { self.cells[0] = 1 }\n}\nfn main() {}",
        Code::AssignToImmutable,
        (3, 20, "self.cells[0] = 1"),
        &[(2, 5)],
    );
}

#[test]
fn element_value_of_the_wrong_type() {
    codes(
        "fn main() {\n    let mut xs = [0]\n    xs[0] = true\n}",
        &[(Code::Mismatch, (3, 13))],
    );
}

#[test]
fn element_of_an_optional_array() {
    let d = diagnostic(
        "fn main() {\n    let mut m: [i64]? = None\n    m[0] = 1\n}",
        Code::MayBeNone,
        (3, 5, "m"),
        &[],
    );
    assert!(d.fixes().is_empty(), "{d:?}");
}

#[test]
fn element_of_a_non_array() {
    diagnostic(
        "fn main() {\n    let mut n = 1\n    n[0] = 2\n}",
        Code::CannotIndex,
        (3, 5, "n"),
        &[],
    );
}

/// An unknown root is reported once, and nothing more about the write.
#[test]
fn element_of_an_unknown_name() {
    codes(
        "fn main() {\n    nope[0] = 1\n}",
        &[(Code::BindingNotFound, (2, 5))],
    );
}

#[test]
fn element_index_of_the_wrong_type() {
    diagnostic(
        "fn f(b: u8) {\n    let mut xs = [0]\n    xs[b] = 1\n}\nfn main() {}",
        Code::Mismatch,
        (3, 8, "b"),
        &[],
    );
}

/// A compound assignment to an element or a field reports what the same statement on a local of
/// the same type reports: the same codes, at the same places relative to the target.
#[test]
fn compound_assignment_reports_as_a_local_would() {
    let types = [
        "string", "string?", "bool", "i64", "i64?", "f64", "f64?", "u8",
    ];
    let values = ["'x'", "c", "1", "\"s\"", "1.5", "true"];
    for ty in types {
        for op in ["+=", "-=", "*="] {
            for value in values {
                let reported = |place: &str, binding: &str| {
                    let src = format!(
                        "class P {{\n    mut f: {ty}\n}}\nfn f(c: char, p: P) {{\n    {binding}\n    {place} {op} {value}\n}}\nfn main() {{}}"
                    );
                    // Line 6 holds the statement; past the target, which starts at column 5,
                    // its columns shift by the target's length.
                    let at = |(line, col): (usize, usize)| match (line, col) {
                        (6, 6..) => (0, col - place.len()),
                        (6, _) => (0, col),
                        _ => (line, col),
                    };
                    let found: Vec<_> = found(&src)
                        .into_iter()
                        .map(|(code, pos, secondary)| {
                            (
                                code,
                                at(pos),
                                secondary.into_iter().map(at).collect::<Vec<_>>(),
                            )
                        })
                        .collect();
                    (found, src)
                };
                let (local, src) = reported("x", "let mut x = p.f");
                for (place, binding) in [("xs[0]", "let mut xs = [p.f]"), ("p.f", "let y = 0")] {
                    let (other, other_src) = reported(place, binding);
                    assert_eq!(other, local, "{other_src}\nagainst\n{src}");
                }
            }
        }
    }
}
