//! The reuse pass: a construction builds a box into a token that every path into it holds.

use std::fmt::Write as _;

use super::*;
use crate::mir::reuse::{MAX_OPEN, available};

const MATCH_REUSE: &str = include_str!("../../../tests/run/match_reuse.sisu");

/// Function `name` of `printed` as printed, its temporaries' numbers kept.
fn raw<'a>(printed: &'a str, name: &str) -> &'a str {
    let start = printed
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("no function {name} in {printed}"));
    let rest = &printed[start..];
    &rest[..rest.find("\n}").map_or(rest.len(), |at| at + 2)]
}

/// The tokens of `f`'s first `take_apart` with tokens, outermost first.
fn tokens(f: &str) -> Vec<&str> {
    let line = f
        .lines()
        .find(|line| line.contains(" = token "))
        .unwrap_or_else(|| panic!("no token in\n{f}"));
    line.split([':', ','])
        .filter(|part| part.contains(" = token "))
        .map(|part| part.trim().split(' ').next().unwrap_or(""))
        .collect()
}

#[test]
fn construction_takes_the_tokens() {
    let printed = built(include_str!("../../../tests/run/enum_alloc_rewrite.sisu"));
    has(
        &printed,
        "swap",
        &["variant Expr.Add(move r#2, move l#1) into [_, _]"],
    );
    let f = function(&printed, "swap");
    assert!(!f.contains("free"), "{f}");
}

/// Neither the loop's `Add` nor the one after the loop takes a token, so both are freed at
/// the arm's end.
#[test]
fn construction_in_a_loop_takes_no_token() {
    let printed = built(MATCH_REUSE);
    let f = function(&printed, "grow");
    assert!(!f.contains(" into ["), "{f}");
    has(
        &printed,
        "grow",
        &[
            "= variant Expr.Add(move acc#4, move _)",
            "_ = variant Expr.Add(move acc#4, move r#3)\n    free _\n    free _\n    goto",
        ],
    );
    assert_eq!(f.matches("free _").count(), 2, "{f}");
}

/// The `Add(a, b)` path builds two `Add`s into the inner and outer tokens, the `other` arm one
/// into the outer tokens, so every `free` is gone.
#[test]
fn inner_match_takes_outer_tokens() {
    let printed = built(MATCH_REUSE);
    has(
        &printed,
        "rotate",
        &[
            "variant Expr.Add(move b#4, move c#2) into [_, _]",
            "variant Expr.Add(move a#3, move _) into [_, _]",
            "variant Expr.Add(move other#5, move c#2) into [_, _]",
        ],
    );
    let f = function(&printed, "rotate");
    assert_eq!(f.matches(" into [").count(), 3, "{f}");
    assert!(!f.contains("free"), "{f}");
}

/// A `Tree` box is larger than an `Expr` box, so a `Tree.Node` takes neither `Expr` token.
#[test]
fn token_of_another_enum_is_not_taken() {
    let printed = built(MATCH_REUSE);
    let f = function(&printed, "convert");
    assert!(!f.contains(" into ["), "{f}");
    assert_eq!(f.matches("free _").count(), 2, "{f}");
}

/// The `Neg` takes the first token on one path. After the join only the second is held on both
/// paths, so the `Add` takes it, and the first token's `free` stays for the other path.
#[test]
fn join_offers_only_tokens_both_paths_hold() {
    let printed = built(MATCH_REUSE);
    let f = raw(&printed, "wrap");
    let [first, second] = tokens(f)[..] else {
        panic!("two tokens in\n{f}")
    };
    for part in [
        format!("variant Expr.Neg(move l#2) into [{first}]"),
        format!(
            "variant Expr.Add(move x#4, move r#3) into [{second}, -]\n    free {first}\n    goto"
        ),
    ] {
        assert!(f.contains(&part), "`{part}` in\n{f}");
    }
    assert_eq!(f.matches("free").count(), 1, "{f}");
}

/// The `Neg` arm frees its token before the join, and the `return` arm reaches no join, so the
/// `Neg` after the `match` has no token to take.
#[test]
fn freed_token_is_not_taken_after_the_join() {
    let printed = built(
        "enum Expr {\n    Num(i64)\n    Neg(Expr)\n}\n\
         fn after(e: Expr) -> Expr {\n    let x = match e {\n        Neg(x) => x\n        \
         other => return other\n    }\n    Expr.Neg(x)\n}\n",
    );
    let f = function(&printed, "after");
    assert!(f.contains("free _\n    goto"), "{f}");
    assert!(!f.contains(" into ["), "{f}");
}

/// `Wrap` is outside `Expr`'s cycle, so `W` holds its `Expr` inline, and an inline field takes
/// no token.
#[test]
fn inline_field_takes_no_token() {
    let printed = built(
        "enum Expr {\n    Num(i64)\n    Neg(Expr)\n}\nenum Wrap {\n    W(Expr)\n}\n\
         fn wrap(e: Expr) -> Wrap {\n    match e {\n        Neg(x) => Wrap.W(x)\n        \
         other => Wrap.W(other)\n    }\n}\n",
    );
    has(&printed, "wrap", &["variant Wrap.W(move x#1)\n    free _"]);
    assert!(!function(&printed, "wrap").contains(" into ["), "{printed}");
}

/// Each of 40 nested `match`es takes two boxes apart, 80 tokens in all, yet no block is
/// offered more than `MAX_OPEN`, and the innermost blocks reach it.
#[test]
fn open_tokens_stay_bounded() {
    const DEPTH: usize = 40;
    let mut src = String::from(
        "enum Expr {\n    Num(i64)\n    Add(Expr, Expr)\n}\nfn f(e: Expr) -> Expr {\n",
    );
    let mut scrutinee = "e".to_owned();
    for level in 0..DEPTH {
        write!(src, "match {scrutinee} {{\nAdd(l{level}, r{level}) => ")
            .expect("writing to a `String` should not fail");
        scrutinee = format!("l{level}");
    }
    src.push_str(&scrutinee);
    for _ in 0..DEPTH {
        src.push_str("\nother => other\n}");
    }
    src.push_str("\n}");
    let program = checked(&src);
    let built = build(&program);
    let f = built
        .functions
        .iter()
        .find(|f| f.name == "f")
        .expect("the program should hold `f`");
    let most = available(f, &program).iter().map(Vec::len).max();
    assert_eq!(most, Some(MAX_OPEN));
}

/// `flip` builds into both tokens when it does not return early, and its `return` edge frees
/// both.
#[test]
fn token_used_on_one_branch_is_freed_on_the_other() {
    let printed = built(include_str!("../../../tests/run/match_take_apart.sisu"));
    has(
        &printed,
        "flip",
        &[
            "variant Expr.Num(0)\n    free _\n    free _\n",
            "return",
            "variant Expr.Add(move r#3, move l#2) into [_, _]\n    goto",
        ],
    );
    assert_eq!(function(&printed, "flip").matches("free").count(), 2);
}
