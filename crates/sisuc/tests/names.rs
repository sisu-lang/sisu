//! What a name means at its position. `sisu check` gives one true answer for each meaning of a
//! name in each place where the program writes it. The names table of the diagnostics foundation
//! spec (step 3) lists them.

use std::process::Command;
use std::thread;

/// One diagnostic record of `sisu check --format json`: the code, the position and the fixes.
struct Record {
    code: String,
    line: u64,
    column: u64,
    fixes: usize,
}

impl Record {
    fn place(&self) -> String {
        format!("{} {}:{}", self.code, self.line, self.column)
    }
}

/// The errors of `sisu check` on `program`, in the printed order.
fn records_of(program: &str) -> Vec<Record> {
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .args(["check", "--format", "json", "-e", program])
        .output()
        .expect("sisu should start");
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("each line should be JSON"))
        .filter(|v| v["kind"] == "diagnostic" && v["severity"] == "error")
        .map(|v| Record {
            code: v["code"].as_str().expect("a code").to_string(),
            line: v["primary"]["line"].as_u64().expect("a line"),
            column: v["primary"]["column"].as_u64().expect("a column"),
            fixes: v["fixes"].as_array().expect("a fixes array").len(),
        })
        .collect()
}

/// Each error of `program` as `"{code} {line}:{column}"`, in the printed order.
fn errors_of(program: &str) -> Vec<String> {
    records_of(program).iter().map(Record::place).collect()
}

/// A program and the errors that it should give.
struct Row {
    name: String,
    program: String,
    errors: Vec<String>,
}

fn row(name: &str, program: &str, errors: &[&str]) -> Row {
    Row {
        name: name.to_string(),
        program: program.to_string(),
        errors: errors.iter().map(ToString::to_string).collect(),
    }
}

/// Runs each row and describes each one that differs. With `codes_only`, a position in an error
/// is not compared.
fn differing(rows: &[Row], codes_only: bool) -> Vec<String> {
    let workers = thread::available_parallelism().map_or(4, usize::from);
    thread::scope(|s| {
        let handles: Vec<_> = rows
            .chunks(rows.len().div_ceil(workers))
            .map(|chunk| {
                s.spawn(move || {
                    let cut = |e: &String| {
                        if codes_only {
                            e.split(' ').next().unwrap_or_default().to_string()
                        } else {
                            e.clone()
                        }
                    };
                    chunk
                        .iter()
                        .filter_map(|r| {
                            let got: Vec<String> = errors_of(&r.program).iter().map(cut).collect();
                            let want: Vec<String> = r.errors.iter().map(cut).collect();
                            (got != want).then(|| format!("{}: got {got:?}, want {want:?}", r.name))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("a worker should not panic"))
            .collect()
    })
}

/// The eight columns of the names table: the use `U` in each position, with `N` the name.
const POSITIONS: [(&str, &str); 8] = [
    ("value", "let _a = N"),
    ("callee", "let _a = N(1)"),
    ("type", "let _a: [N] = []"),
    ("pattern", "match 1 { N => {}\n _ => {} }"),
    ("path", "let _a = N.x"),
    ("path call", "let _a = N.x(1)"),
    ("type args", "let _a = N<i64>(1)"),
    ("path type args", "let _a = N<i64>.x(1)"),
];

/// A meaning of the names table. `shape` has a `U` for the use. `spelled` replaces `N`; with
/// `None`, the shape keeps its name. `declaration` lists the errors of the declaration, and
/// `codes` has the codes at each position.
struct Meaning {
    name: &'static str,
    shape: &'static str,
    spelled: Option<&'static str>,
    declaration: &'static [&'static str],
    codes: [&'static str; 8],
}

#[rustfmt::skip]
const MEANINGS: [Meaning; 24] = [
    Meaning { name: "local", shape: "fn f(N: i64) { U }", spelled: None, declaration: &[],
        codes: ["ok", "E0052", "E0030", "E0125", "E0063", "E0063", "E0137", "E0137"] },
    Meaning { name: "constant", shape: "const N = 1\nfn f() { U }", spelled: None, declaration: &[],
        codes: ["ok", "E0052", "E0030", "E0126", "E0063", "E0063", "E0137", "E0137"] },
    Meaning { name: "type parameter", shape: "fn f<N>() { U }", spelled: None, declaration: &[],
        codes: ["E0105", "E0052", "ok", "E0125", "E0105", "E0105", "E0052", "E0105"] },
    Meaning { name: "function", shape: "fn N() -> i64 { 1 }\nfn f() { U }", spelled: None, declaration: &[],
        codes: ["E0105", "E0053", "E0030", "E0125", "E0105", "E0105", "E0053,E0137", "E0105,E0137"] },
    Meaning { name: "class", shape: "class N {}\nfn f() { U }", spelled: None, declaration: &[],
        codes: ["E0105", "E0064", "ok", "E0125", "E0105", "E0105", "E0137,E0064", "E0105,E0137"] },
    Meaning { name: "enum", shape: "enum N { A }\nfn f() { U }", spelled: None, declaration: &[],
        codes: ["E0105", "E0105", "ok", "E0125", "E0118", "E0118", "E0105,E0137", "E0137,E0118"] },
    Meaning { name: "enum beside a local of its name", shape: "enum N { A }\nfn f(N: i64) { U }", spelled: None, declaration: &[],
        codes: ["E0116", "E0116,E0052", "E0116", "E0116,E0125", "E0116,E0118", "E0116,E0118", "E0116,E0052,E0137", "E0116,E0137,E0118"] },
    Meaning { name: "interface", shape: "interface N {}\nfn f() { U }", spelled: None, declaration: &[],
        codes: ["E0105", "E0105", "E0147", "E0125", "E0105", "E0105", "E0105", "E0105"] },
    Meaning { name: "alias of i64", shape: "type N = i64\nfn f() { U }", spelled: None, declaration: &[],
        codes: ["E0105", "E0054", "ok", "E0125", "E0063", "E0105", "E0137,E0054", "E0105,E0137"] },
    Meaning { name: "alias of an enum", shape: "enum E { A }\ntype N = E\nfn f() { U }", spelled: None, declaration: &[],
        codes: ["E0105", "E0105", "ok", "E0125", "E0118", "E0118", "E0105,E0137", "E0137,E0118"] },
    Meaning { name: "alias of a class", shape: "class C { v: i64 }\ntype N = C\nfn f() { U }", spelled: None, declaration: &[],
        codes: ["E0105", "E0065", "ok", "E0125", "E0105", "E0105", "E0137,E0065", "E0105,E0137"] },
    Meaning { name: "built-in type i64", shape: "fn f() { U }", spelled: Some("i64"), declaration: &[],
        codes: ["E0105", "E0054", "ok", "ok", "E0063", "E0105", "E0137,E0054", "E0105,E0137"] },
    Meaning { name: "built-in type f64", shape: "fn f() { U }", spelled: Some("f64"), declaration: &[],
        codes: ["E0105", "ok", "ok", "ok", "E0063", "E0105", "E0137", "E0105,E0137"] },
    Meaning { name: "Option", shape: "fn f() { U }", spelled: Some("Option"), declaration: &[],
        codes: ["E0105", "E0105", "E0137", "E0026", "E0118", "E0118", "E0105", "E0118"] },
    Meaning { name: "print", shape: "fn f() { U }", spelled: Some("print"), declaration: &[],
        codes: ["E0105", "E0043", "E0030", "ok", "E0105", "E0105", "E0043,E0137", "E0105,E0137"] },
    Meaning { name: "Some", shape: "fn f() { U }", spelled: Some("Some"), declaration: &[],
        codes: ["E0119", "ok", "E0030", "E0026", "E0119", "E0119", "ok", "E0119"] },
    Meaning { name: "Ok", shape: "fn f() { U }", spelled: Some("Ok"), declaration: &[],
        codes: ["E0119", "E0050", "E0030", "E0026", "E0119", "E0119", "E0137", "E0119"] },
    Meaning { name: "associated type", shape: "interface I { type N\n fn m(self) }\nclass C: I {\n type N = i64\n fn m(self) { U }\n}", spelled: None, declaration: &[],
        codes: ["E0105", "E0052", "ok", "E0125", "E0105", "E0105", "E0052", "E0105"] },
    Meaning { name: "poisoned item", shape: "enum N\n{ A }\nfn f() { U }", spelled: None, declaration: &["E0004"],
        codes: ["ok", "ok", "ok", "ok", "ok", "ok", "ok", "ok"] },
    Meaning { name: "poisoned enum hidden by a local", shape: "enum N\n{ A }\nfn f(N: i64) { U }", spelled: None, declaration: &["E0004"],
        codes: ["ok", "ok", "ok", "ok", "ok", "ok", "E0137", "E0137"] },
    Meaning { name: "poisoned class hidden by a local", shape: "class N\n{ }\nfn f(N: i64) { U }", spelled: None, declaration: &["E0004"],
        codes: ["ok", "ok", "ok", "ok", "E0063", "E0063", "E0137", "E0137"] },
    Meaning { name: "poisoned constant hidden by a local", shape: "const N =\n\nfn f(N: i64) { U }", spelled: None, declaration: &["E0011"],
        codes: ["ok", "ok", "ok", "ok", "E0063", "E0063", "E0137", "E0137"] },
    Meaning { name: "alias of a poisoned item", shape: "enum Z\n{ A }\ntype N = Z\nfn f() { U }", spelled: None, declaration: &["E0004"],
        codes: ["ok", "ok", "ok", "ok", "ok", "ok", "ok", "ok"] },
    Meaning { name: "unknown", shape: "fn f() { U }", spelled: None, declaration: &[],
        codes: ["E0033", "E0052", "E0030", "E0125", "E0033", "E0033", "E0052", "E0033"] },
];

/// GIVEN a name with each meaning of the names table, used at each of the eight positions
/// WHEN `sisu check` runs on each of the programs
/// THEN each gives the codes of the table, in order, after the errors of the declaration
/// AND a type parameter or an associated type used as a value gives E0105, also on a path
/// AND a poisoned enum under a local gives E0137 on a path with type args.
#[test]
fn each_meaning_at_each_position_should_give_its_code() {
    let mut rows = Vec::new();
    for m in &MEANINGS {
        let name = m.spelled.unwrap_or("N");
        for ((position, user), codes) in POSITIONS.iter().zip(m.codes) {
            let program = m.shape.replace('U', user).replace('N', name) + "\nfn main() {}";
            let mut want: Vec<&str> = m.declaration.to_vec();
            want.extend(codes.split(',').filter(|c| *c != "ok"));
            rows.push(row(&format!("{} at {position}", m.name), &program, &want));
        }
    }
    let wrong = differing(&rows, true);
    assert!(
        wrong.is_empty(),
        "{} of {} cells differ:\n{}",
        wrong.len(),
        rows.len(),
        wrong.join("\n")
    );
}

/// GIVEN a program with a name that a local, a poisoned item, an interface, a type or a language
/// name takes
/// AND the name is at one of the positions
/// WHEN `sisu check` runs on it
/// THEN the errors are exactly the true ones, with their positions.
#[test]
#[expect(clippy::too_many_lines, reason = "one table of rows")]
fn a_name_at_its_position_should_give_one_true_answer() {
    let rows = [
        row(
            "P2 (control)",
            "fn f(Bar: string) {\n    match zz {\n        Bar => {\n            let _a: i64 = Bar\n        }\n        _ => {}\n    }\n}\n\nfn main() {}",
            &["E0033 2:11", "E0032 4:27"],
        ),
        row(
            "P3",
            "enum Result\n{ Ok }\n\nfn main() {\n    match zz {\n        Ok => {\n            Ok: loop {\n                break Ok\n            }\n        }\n        _ => {}\n    }\n}",
            &["E0026 1:6", "E0004 2:1", "E0033 5:11"],
        ),
        row(
            "P4",
            "const i64 =\n\nfn main() {\n    let _a = i64.x\n}",
            &["E0026 1:7", "E0011 3:1"],
        ),
        row(
            "P4b",
            "const i64 =\n\nfn main() {\n    let _a = i64.m<i64>()\n}",
            &["E0026 1:7", "E0011 3:1"],
        ),
        row(
            "P4c",
            "const Option =\n\nfn main() {\n    let _a = Option.m()\n}",
            &["E0026 1:7", "E0011 3:1"],
        ),
        row(
            "P5",
            "enum Foo\n{ A }\n\nfn f(Foo: i64) {\n    let _a = Foo<i64>.x(1)\n}\n\nfn main() {}",
            &["E0004 2:1", "E0137 5:14"],
        ),
        row(
            "option args",
            "const Option =\n\nfn main() {\n    let _a = Option.m<i64>()\n}",
            &["E0026 1:7", "E0011 3:1"],
        ),
        row(
            "bool call (control)",
            "fn main() {\n    let _a = bool(1)\n}",
            &["E0105 2:14"],
        ),
        row(
            "alias enum local (control)",
            "enum E { A }\ntype N = E\nfn f(N: i64) { let _a = N.A }\nfn main() {}",
            &["E0116 3:6"],
        ),
        row(
            "alias option local (control)",
            "type N = i64?\nfn f(N: i64) { let _a = N.Some(1) }\nfn main() {}",
            &["E0116 2:6"],
        ),
        row(
            "alias class local (control)",
            "class C { v: i64 }\ntype N = C\nfn f(N: i64) { let _a = N.v }\nfn main() {}",
            &["E0063 3:27"],
        ),
        row(
            "bool args (control)",
            "fn main() {\n    let _a = bool<i64>(1)\n}",
            &["E0105 2:14", "E0137 2:18"],
        ),
        row(
            "interface call (control)",
            "interface N {}\nfn f() {\n    let _a = N(1)\n}\nfn main() {}",
            &["E0105 3:14"],
        ),
        row(
            "interface method (control)",
            "interface N {}\nclass C {\n    fn N(self) {}\n}\nfn main() {\n    let _a = N(C())\n}",
            &["E0105 6:14"],
        ),
        row(
            "interface Some (control)",
            "interface Some {}\nfn main() {\n    let _a = Some(1)\n}",
            &["E0026 1:11", "E0105 3:14"],
        ),
        row(
            "assign class (control)",
            "class C {}\nfn main() {\n    C = 1\n}",
            &["E0105 3:5"],
        ),
        row(
            "assign parameter",
            "fn f<T>() {\n    T = 1\n}\nfn main() {}",
            &["E0105 2:5"],
        ),
        row(
            "P1",
            "enum Foo\n{ A }\n\ntype Y = Foo\n\nfn main() {\n    match 1 {\n        Y => {}\n        _ => {}\n    }\n}",
            &["E0004 2:1"],
        ),
        row(
            "P1b",
            "enum Foo\n{ A }\n\ntype Y = Foo\n\nfn main() {\n    match 1 {\n        Y(x) => {}\n        _ => {}\n    }\n}",
            &["E0004 2:1"],
        ),
        row(
            "P1c",
            "enum Foo\n{ A }\n\ntype Y = Foo\n\nfn main() {\n    let _a = Y\n}",
            &["E0004 2:1"],
        ),
        row(
            "P6",
            "interface Eq\n{}\nconst Eq = 1\n\nfn main() {\n    let p = Eq\n    print(p)\n}",
            &["E0026 1:11", "E0004 2:1", "E0026 3:7", "E0034 3:7"],
        ),
        row(
            "P6b",
            "interface Eq\n{}\nclass Eq {}\nfn g(x: Eq) {}\nfn main() {}",
            &["E0026 1:11", "E0004 2:1", "E0026 3:7", "E0034 3:7"],
        ),
        row(
            "R1",
            "class A {}\nconst A = 1\n\nfn main() {\n    let _a = A\n}",
            &["E0034 2:7"],
        ),
        row(
            "R2",
            "const A = 1\nclass A {}\n\nfn main() {\n    let _a: A = A()\n}",
            &["E0034 2:7"],
        ),
        row(
            "R3",
            "fn A(x: i64) {}\nfn A(x: string) {}\n\nfn main() {\n    A(\"s\")\n}",
            &["E0034 2:4"],
        ),
        row(
            "R4",
            "enum A { V }\nfn A() {}\n\nfn main() {\n    A()\n}",
            &["E0034 2:4"],
        ),
        row(
            "R5",
            "class A { v: i64 }\nclass A { w: i64 }\n\nfn main() {\n    let a = A(w: 1)\n    print(a.w)\n}",
            &["E0034 2:7"],
        ),
        row(
            "R6",
            "fn A(x: i64) {}\nfn A(x: string) {}\n\nfn main() {\n    A(\"s\")\n    print(1 + true)\n}",
            &["E0034 2:4", "E0032 6:15"],
        ),
        row(
            "R7",
            "interface I {}\ninterface I {}\nclass C {}\nfn g<T: I>(x: T) { x.m() }\nfn main() { g(C()) }",
            &["E0034 2:11"],
        ),
        row(
            "A1",
            "enum Z\n{ A }\ntype N = [Z]\nfn main() {\n    let _a = N\n}",
            &["E0004 2:1"],
        ),
        row(
            "A2",
            "enum Z\n{ A }\ntype M = Z\ntype N = M\nfn main() {\n    let _a = N\n}",
            &["E0004 2:1"],
        ),
        row(
            "A3",
            "type N = Missing\nfn main() {\n    let _a = N\n}",
            &["E0030 1:10"],
        ),
        row(
            "A4",
            "type N = N\nfn main() {\n    let _a = N\n}",
            &["E0161 1:6"],
        ),
        row(
            "A5",
            "class R {}\nclass R {}\ntype N = R\nfn main() {\n    let _a = N(1)\n}",
            &["E0034 2:7"],
        ),
        row(
            "I1",
            "interface Eq {}\ninterface Eq {}\nfn g(x: Eq) {}\nfn main() {}",
            &["E0026 1:11", "E0026 2:11", "E0034 2:11"],
        ),
        row(
            "I2 (control)",
            "interface Eq {}\ninterface Eq {}\nfn g<T: Eq>(x: T) {}\nfn main() {}",
            &["E0026 1:11", "E0026 2:11", "E0034 2:11"],
        ),
        row(
            "I3 (control)",
            "interface Eq {}\nclass Eq {}\nfn g<T: Eq>(x: T) {}\nclass R: Eq {}\nfn main() {}",
            &["E0026 1:11", "E0026 2:7", "E0034 2:7", "E0152 4:10"],
        ),
        row(
            "I4 (control)",
            "interface Eq {}\ninterface Eq {}\nclass C: Eq {\n    fn eq(self, other: C) -> bool { true }\n}\nfn main() {\n    print(C() == C())\n}",
            &["E0026 1:11", "E0026 2:11", "E0034 2:11"],
        ),
        row(
            "I5 (control)",
            "interface Eq {}\nclass Eq {}\nfn g<T: Eq>(x: T) -> bool { x == x }\nfn main() {}",
            &["E0026 1:11", "E0026 2:7", "E0034 2:7"],
        ),
        row(
            "I6 (control)",
            "interface Ord {}\ninterface Ord {}\nfn g<T: Ord>(x: T) -> bool { x < x }\nfn main() {}",
            &["E0026 1:11", "E0026 2:11", "E0034 2:11"],
        ),
        row(
            "I7 (control)",
            "interface Display {}\ninterface Display {}\nclass C: Display {\n    fn display(self) -> string { \"\" }\n}\nfn main() {\n    print(C())\n}",
            &["E0026 1:11", "E0026 2:11", "E0034 2:11"],
        ),
        row(
            "I8 (control)",
            "interface Eq {}\ninterface Eq {}\nfn g(x: any Eq) {}\nfn main() {}",
            &["E0026 1:11", "E0026 2:11", "E0034 2:11", "E0164 3:13"],
        ),
        row(
            "alias args call (control)",
            "enum Z\n{ A }\ntype N = Z\nfn main() {\n    let _a = N<Missing>(1)\n}",
            &["E0004 2:1"],
        ),
    ];
    let wrong = differing(&rows, false);
    assert!(
        wrong.is_empty(),
        "{} of {} rows differ:\n{}",
        wrong.len(),
        rows.len(),
        wrong.join("\n")
    );
}

/// GIVEN the program P7, where a type parameter is a value
/// WHEN `sisu check` runs on it
/// THEN the only error is E0105 at 2:23
/// AND it offers no fix.
#[test]
fn a_type_parameter_used_as_a_value_should_give_one_error_and_no_fix() {
    let records = records_of(
        "interface Bp { fn bp(self, x: i64) }\nfn q<T: Bp>(c: i64) { T.bp(c) }\nfn main() {}",
    );
    let seen: Vec<_> = records.iter().map(|r| (r.place(), r.fixes)).collect();
    assert_eq!(seen, [("E0105 2:23".to_string(), 0)]);
}

/// GIVEN the program P8, where an associated type is a function
/// WHEN `sisu check` runs on it
/// THEN the only error is E0052 at 5:27
/// AND it offers no fix.
#[test]
fn an_associated_type_used_as_a_function_should_give_one_error_and_no_fix() {
    let records = records_of(
        "interface I { type Item\n fn m(self) }\nclass C: I {\n    type Item = i64\n    fn m(self) { let _c = Item(1) }\n}\nfn main() {}",
    );
    let seen: Vec<_> = records.iter().map(|r| (r.place(), r.fixes)).collect();
    assert_eq!(seen, [("E0052 5:27".to_string(), 0)]);
}

/// GIVEN `let x = nothing()` and then `print(x)`
/// WHEN `sisu check` runs on it
/// THEN the only error is E0052 at the call
/// AND no error says that `x` does not exist.
#[test]
fn a_local_with_no_type_should_stay_a_found_name() {
    assert_eq!(
        errors_of("fn main() {\n    let x = nothing()\n    print(x)\n}"),
        ["E0052 2:13"]
    );
}

/// GIVEN a path `N.x` of an enum, an alias of an enum or a class, `f64`, `Option` or `math`
/// WHEN `sisu check` runs on each program
/// THEN each gives the errors of its row of the path order, with no more
/// AND an alias of an enum or of `Option` wins over a local of its name.
#[test]
fn a_path_should_keep_its_order() {
    let rows = [
        row(
            "enum beside a local",
            "enum N { A }\nfn f(N: i64) { let _a = N.A }\nfn main() {}",
            &["E0116 2:6"],
        ),
        row(
            "alias of an enum",
            "enum E { A }\ntype N = E\nfn main() {\n    let _a = N.A\n}",
            &[],
        ),
        row(
            "alias of an enum, a call",
            "enum E { V(i64) }\ntype N = E\nfn main() {\n    let _a = N.V(1)\n}",
            &[],
        ),
        row(
            "alias of an enum, no variant",
            "enum E { A }\ntype N = E\nfn main() {\n    let _a = N.nope\n}",
            &["E0118 4:16"],
        ),
        row(
            "alias of a class",
            "class C { v: i64 }\ntype N = C\nfn main() {\n    let _a = N.v\n}",
            &["E0105 4:14"],
        ),
        row(
            "a constant of f64",
            "fn main() {\n    let _a = f64.pi\n}",
            &[],
        ),
        row(
            "Option, a variant",
            "fn main() {\n    let _a = Option.Some(1)\n}",
            &[],
        ),
        row(
            "Option, None",
            "fn main() {\n    let _a: i64? = Option.None\n}",
            &[],
        ),
        row(
            "the math idiom",
            "fn main() {\n    let _a = math.pi\n}",
            &["E0106 2:14"],
        ),
        row(
            "alias variant call (control)",
            "enum E { V(i64) }\ntype N = E\nfn f(N: i64) {\n    let _a = N.V(1)\n}\nfn main() {}",
            &["E0116 3:6"],
        ),
        row(
            "alias none (control)",
            "type N = i64?\nfn f(N: i64) {\n    let _a: i64? = N.None\n}\nfn main() {}",
            &["E0116 2:6"],
        ),
    ];
    let wrong = differing(&rows, false);
    assert!(
        wrong.is_empty(),
        "{} of {} rows differ:\n{}",
        wrong.len(),
        rows.len(),
        wrong.join("\n")
    );
}

/// GIVEN a class, an enum, an interface or an alias with the name `Some`, `Ok` or `Err`
/// WHEN `sisu check` runs on a program that assigns to the name
/// AND the program may add to it, pass it as a `mut` argument or assign to an index of it
/// THEN the errors are the declaration's E0026 and an E0105, as for any other item of that kind.
#[test]
fn an_assignment_to_a_prelude_names_item_should_keep_its_wrong_kind_error() {
    let declarations = [
        ("class", "class N {}"),
        ("enum", "enum N { A }"),
        ("interface", "interface N {}"),
        ("alias of a number", "type N = i64"),
        ("alias of a class", "class C {}\ntype N = C"),
    ];
    let uses = [
        ("assignment", "N = 1"),
        ("compound assignment", "N += 1"),
        ("mut argument", "set(mut N)"),
        ("index assignment", "N[0] = 1"),
    ];
    let mut rows = Vec::new();
    for name in ["Some", "Ok", "Err"] {
        for (kind, declaration) in declarations {
            for (form, user) in uses {
                let program =
                    format!("{declaration}\nfn set(mut x: i64) {{}}\nfn main() {{ {user} }}\n")
                        .replace('N', name);
                rows.push(row(
                    &format!("{kind} {name}, {form}"),
                    &program,
                    &["E0026 1:1", "E0105 1:1"],
                ));
            }
        }
    }
    let wrong = differing(&rows, true);
    assert!(
        wrong.is_empty(),
        "{} of {} rows differ:\n{}",
        wrong.len(),
        rows.len(),
        wrong.join("\n")
    );
}

/// Asserts that no row of `rows` differs from its errors.
fn assert_rows(rows: &[Row]) {
    let wrong = differing(rows, false);
    assert!(
        wrong.is_empty(),
        "{} of {} rows differ:\n{}",
        wrong.len(),
        rows.len(),
        wrong.join("\n")
    );
}

/// GIVEN a top-level name that a program declares twice
/// AND a use of the name before or after the second declaration, at any position
/// WHEN `sisu check` runs on it
/// THEN the errors are the redeclaration's own, and a use gives no more
/// AND a language interface's redeclaration gives the same, with its E0026 on each declaration.
#[test]
fn a_redeclared_name_should_leave_only_its_error() {
    let eq = "interface Eq {}\ninterface Eq {}\nfn main() {\n";
    let eq_twice = ["E0026 1:11", "E0026 2:11", "E0034 2:11"];
    let rows = [
        row(
            "forward fn (control)",
            "fn main() { A() }\nfn A() {}\nfn A(x: i64) {}",
            &["E0034 3:4"],
        ),
        row(
            "forward alias",
            "type N = R\nclass R {}\nclass R {}\nfn main() {\n    let _a = N(1)\n}",
            &["E0034 3:7"],
        ),
        row(
            "main twice (control)",
            "fn main() {}\nfn main() {}",
            &["E0034 2:4"],
        ),
        row(
            "three fns (control)",
            "fn A() {}\nfn A() {}\nfn A() {}\nfn main() {}",
            &["E0034 2:4", "E0034 3:4"],
        ),
        row(
            "variant of twice",
            "enum E { A }\nenum E { B }\nfn main() {\n    let _a = E.B\n    let _b = E.A\n}",
            &["E0034 2:6"],
        ),
        row(
            "const twice",
            "const A = 1\nconst A = 2\nconst B = A + true\nfn main() { print(B) }",
            &["E0034 2:7"],
        ),
        row(
            "header twice (control)",
            "interface I {}\ninterface I {}\nclass C: I {}\nfn main() { let _c = C() }",
            &["E0034 2:11"],
        ),
        row(
            "broken Eq second",
            "interface Eq {}\ninterface Eq\n{}\nfn f(x: Eq) {}\nfn main() {}",
            &["E0026 1:11", "E0026 2:11", "E0034 2:11", "E0004 3:1"],
        ),
        row(
            "broken Eq first",
            "interface Eq\n{}\ninterface Eq {}\nfn f(x: Eq) {}\nfn main() {}",
            &["E0026 1:11", "E0004 2:1", "E0026 3:11", "E0034 3:11"],
        ),
        row("Eq value", &format!("{eq}    let _a = Eq\n}}"), &eq_twice),
        row("Eq call", &format!("{eq}    let _a = Eq(1)\n}}"), &eq_twice),
        row("Eq path", &format!("{eq}    let _a = Eq.x\n}}"), &eq_twice),
        row(
            "Eq pattern",
            &format!("{eq}    match 1 {{\n        Eq => {{}}\n        _ => {{}}\n    }}\n}}"),
            &eq_twice,
        ),
        row(
            "duplicate class read",
            "class C {}\nclass C {}\nfn main() { let p = C }\n",
            &["E0034 2:7"],
        ),
        row(
            "duplicate fn read",
            "fn f() {}\nfn f() {}\nfn main() { let p = f }\n",
            &["E0034 2:4"],
        ),
        row(
            "duplicate enum called",
            "enum E { A }\nenum E { B }\nfn main() { E(1) }\n",
            &["E0034 2:6"],
        ),
    ];
    assert_rows(&rows);
}

/// GIVEN a use that a poisoned name cannot make untrue: type args on a failed alias, a member
/// defined twice, a parameter that hides a redeclared name, or one broken definition
/// WHEN `sisu check` runs on it
/// THEN the true error stays.
#[test]
fn an_error_a_poisoned_name_cannot_remove_should_stay() {
    let rows = [
        row(
            "alias args (control)",
            "enum Z\n{ A }\ntype N = Z\nfn f(x: N<Missing>) {}\nfn main() {}",
            &["E0004 2:1", "E0137 4:10"],
        ),
        row(
            "one Eq (control)",
            "interface Eq {}\nfn g(x: Eq) {}\nfn main() {}",
            &["E0026 1:11", "E0147 2:9"],
        ),
        row(
            "method twice (control)",
            "class C {\n    fn m(self) {}\n    fn m(self, x: i64) {}\n}\nfn main() {\n    C().m(true)\n}",
            &["E0034 3:8", "E0053 6:5"],
        ),
        row(
            "field twice (control)",
            "class C {\n    v: i64\n    v: string\n}\nfn main() {\n    let _a: string = C(v: 1).v\n}",
            &["E0034 3:5", "E0032 6:22"],
        ),
        row(
            "param hides (control)",
            "fn A() {}\nfn A() {}\nfn f(A: i64) { print(A + true) }\nfn main() {}",
            &["E0034 2:4", "E0032 3:26"],
        ),
        row(
            "one broken Eq (control)",
            "interface Eq\n{}\nfn f(x: Eq) {}\nfn main() {}",
            &["E0026 1:11", "E0004 2:1", "E0147 3:9"],
        ),
    ];
    assert_rows(&rows);
}

/// GIVEN a generic alias, class, enum or interface that a program declares twice, or writes broken
/// AND a use that writes `Missing`, a type that nothing declares, as its type argument
/// WHEN `sisu check` runs on it, at a type or a bound
/// THEN the error of the argument stays, as nothing a repair does can make it true
/// AND no error for the number of arguments or for a bound comes from the poisoned name
/// AND a call of the name stays as plan call 15 says: only the declaration's error.
#[test]
fn the_type_args_of_a_poisoned_name_should_keep_their_errors() {
    let rows = [
        row(
            "alias, type",
            "type N<T> = [T]\ntype N<T> = [T]\nfn g(x: N<Missing>) {}\nfn main() {}",
            &["E0034 2:6", "E0030 3:11"],
        ),
        row(
            "class, type",
            "class N<T> {}\nclass N<T> {}\nfn g(x: N<Missing>) {}\nfn main() {}",
            &["E0034 2:7", "E0030 3:11"],
        ),
        row(
            "enum, type",
            "enum N<T> { A }\nenum N<T> { B }\nfn g(x: N<Missing>) {}\nfn main() {}",
            &["E0034 2:6", "E0030 3:11"],
        ),
        row(
            "interface, type",
            "interface N<T> {}\ninterface N<T> {}\nfn g(x: N<Missing>) {}\nfn main() {}",
            &["E0034 2:11", "E0030 3:11"],
        ),
        row(
            "interface, bound",
            "interface N<T> {}\ninterface N<T> {}\nfn g<U: N<Missing>>(x: U) {}\nfn main() {}",
            &["E0034 2:11", "E0030 3:11"],
        ),
        row(
            "class, bound",
            "class N<T> {}\nclass N<T> {}\nfn g<U: N<Missing>>(x: U) {}\nfn main() {}",
            &["E0034 2:7", "E0030 3:11"],
        ),
        row(
            "broken class, type",
            "class N<T>\n{}\nfn g(x: N<Missing>) {}\nfn main() {}",
            &["E0004 2:1", "E0030 3:11"],
        ),
        row(
            "class, callee (control)",
            "class N<T> {}\nclass N<T> {}\nfn main() { let _a = N<Missing>(1) }",
            &["E0034 2:7"],
        ),
    ];
    assert_rows(&rows);
}

/// GIVEN an alias whose target fails its bound where the target is declared
/// AND a second alias that names the first, directly or inside an array
/// WHEN `sisu check` runs on a read of the second alias
/// THEN the only error is the bound's E0154, as for the first alias.
#[test]
fn an_alias_of_an_alias_with_a_failed_bound_should_give_only_that_error() {
    let items = "interface I {}\nclass C<T: I> {}\ntype M = C<i64>\n";
    let rows = [
        row(
            "direct (control)",
            &format!("{items}fn main() {{ let x = M }}"),
            &["E0154 3:12"],
        ),
        row(
            "chain",
            &format!("{items}type N = M\nfn main() {{ let x = N }}"),
            &["E0154 3:12"],
        ),
        row(
            "chain in an array",
            &format!("{items}type N = [M]\nfn main() {{ let x = N }}"),
            &["E0154 3:12"],
        ),
    ];
    assert_rows(&rows);
}
