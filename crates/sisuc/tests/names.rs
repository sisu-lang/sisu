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

/// The seven columns of the names table: the use `U` in each position, with `N` the name.
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
const MEANINGS: [Meaning; 23] = [
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
