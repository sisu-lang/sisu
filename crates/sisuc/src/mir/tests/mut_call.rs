//! Copy-in and copy-out of a `mut` argument that is a local, in the mir.

use std::fmt::Write as _;

use super::*;
use crate::mir::lower::{OUT_STEPS, SET_OUT_STEPS};

/// S34's `f`: an early `return` and the end of the body.
const EARLY_RETURN: &str = "fn f(mut xs: [i64], stop: bool) {\n    xs[0] = 1\n    if stop {\n        return\n    }\n    xs[0] = 2\n}";

/// S1: `set` changes the caller's local.
const SET: &str = "fn set(mut xs: [i64]) {\n    xs[0] = 9\n}\n\
    fn main() {\n    let mut xs = [1, 2]\n    set(mut xs)\n    print(xs)\n}";

/// GIVEN `fn f(mut xs: [i64], stop: bool)` that returns early, as in S34
/// WHEN its mir is built
/// THEN each `return` has a `set_out 0` before it in its block
#[test]
fn mut_parameter_should_set_its_out_at_every_return() {
    let printed = built(EARLY_RETURN);
    let f = function(&printed, "f");
    let returning: Vec<&str> = f
        .split("\n  bb")
        .filter(|block| block.contains("\n    return"))
        .collect();
    assert_eq!(returning.len(), 2, "{f}");
    for block in returning {
        let at = block.find("\n    return").expect("the block returns");
        assert!(block[..at].contains("set_out 0"), "{f}");
    }
}

/// GIVEN S1, whose `xs` is a local that the call changes
/// WHEN the mir of `main` is built
/// THEN `xs` moves into a temporary, the call moves the temporary in, `out 0` puts the final
/// value back into it, and the store-back moves it into `xs`
#[test]
fn mut_local_should_move_into_the_call() {
    let printed = built(SET);
    has(
        &printed,
        "main",
        &["= move xs#", "call set(move _)", "_ = out 0"],
    );
    let main = function(&printed, "main");
    let after = &main[main.find("= out 0").expect("an `out` follows the call")..];
    assert!(
        after
            .lines()
            .any(|line| line.trim_start().starts_with("xs#") && line.ends_with("= move _")),
        "{main}"
    );
}

/// GIVEN `show`, whose `mut` parameter the body only reads
/// WHEN its mir is built
/// THEN the signature line prints no `borrow` for the parameter
#[test]
fn mut_parameter_should_never_be_borrowed() {
    let printed = built(
        "fn show(mut xs: [i64], ys: [i64]) {\n    print(xs.len())\n    print(ys.len())\n}\n\
         fn main() {\n    let mut xs = [1]\n    show(mut xs, [2])\n    print(xs)\n}",
    );
    signature(&printed, "show", "fn show(xs#0: [i64], ys#1: borrow [i64])");
}

/// GIVEN `look`, whose body only reads its `mut` parameter and its plain one
/// WHEN `params::assumed` guesses the borrowed parameters
/// THEN the plain parameter is borrowed and the `mut` one is not
#[test]
fn assumed_should_not_borrow_a_mut_parameter() {
    let program = checked(
        "fn look(mut xs: [i64], ys: [i64]) {\n    print(xs.len())\n    print(ys.len())\n}\n\
         fn main() {\n    let mut xs = [1]\n    look(mut xs, [2])\n    print(xs)\n}",
    );
    let look = program
        .functions
        .iter()
        .position(|f| f.name == "look")
        .expect("the program has `look`");
    assert_eq!(super::super::params::assumed(&program)[look], [false, true]);
}

/// Asserts that the signature of `name` in `printed` starts with `head`.
fn signature(printed: &str, name: &str, head: &str) {
    let f = function(printed, name);
    assert!(f.starts_with(head), "`{head}` in\n{f}");
}

/// `n` lines, each written by `line` into the string with its number.
fn lines(n: usize, line: impl Fn(&mut String, usize)) -> String {
    (0..n).fold(String::new(), |mut out, i| {
        line(&mut out, i);
        out
    })
}

/// GIVEN one call with `n` `mut` arguments, for `n` = 100, 200 and 300
/// WHEN the call is lowered
/// THEN the arguments that lowering compares with a temporary grow in proportion to `n`, as a
/// search of all the arguments for each `mut` argument does not
#[test]
fn mut_call_should_find_its_temporaries_in_linear_steps() {
    let steps = |n: usize| {
        let params: Vec<String> = (0..n).map(|i| format!("mut a{i}: i64")).collect();
        let bumps = lines(n, |out, i| writeln!(out, "    a{i} += 1").unwrap());
        let locals = lines(n, |out, i| writeln!(out, "    let mut v{i} = 0").unwrap());
        let given: Vec<String> = (0..n).map(|i| format!("mut v{i}")).collect();
        let shown = lines(n, |out, i| writeln!(out, "    print(v{i})").unwrap());
        let src = format!(
            "fn many({}) {{\n{bumps}}}\nfn main() {{\n{locals}    many({})\n{shown}}}",
            params.join(", "),
            given.join(", ")
        );
        let program = checked(&src);
        OUT_STEPS.set(0);
        drop(lowered_of(&program));
        OUT_STEPS.get()
    };
    let [one, two, three] = [100, 200, 300].map(steps);
    assert_near_linear(one, two, three);
}

/// GIVEN a function with `n` plain parameters, one `mut` parameter and `n` early returns, for `n`
/// = 100, 200 and 300
/// WHEN it is lowered
/// THEN the parameters that `set_outs` reads grow in proportion to `n`, as a scan of every
/// parameter at each return does not
#[test]
fn set_outs_should_read_only_the_mut_parameters() {
    let steps = |n: usize| {
        let plain = lines(n, |out, i| write!(out, "p{i}: i64, ").unwrap());
        let returns = lines(n, |out, i| {
            write!(out, "    if p{i} == 1 {{\n        return\n    }}\n").unwrap();
        });
        let zeros = "0, ".repeat(n);
        let src = format!(
            "fn wide({plain}mut xs: [i64]) {{\n{returns}    xs[0] = 1\n}}\n\
             fn main() {{\n    let mut xs = [0]\n    wide({zeros}mut xs)\n    print(xs)\n}}"
        );
        let program = checked(&src);
        SET_OUT_STEPS.set(0);
        drop(lowered_of(&program));
        SET_OUT_STEPS.get()
    };
    let [one, two, three] = [100, 200, 300].map(steps);
    assert_near_linear(one, two, three);
}
