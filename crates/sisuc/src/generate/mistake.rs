//! Property 7, the single-mistake test: the sites where the generator knows the errors of one
//! edit, and the choice of at most one mistake of each kind. The writer records each site while it
//! writes the text (`Writer::record`); `choose` turns the sites of a program into edits.

use std::ops::Range;

/// The kinds of property 7, in the order of the spec's table. The bound kind is property 6.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Task 1, Step 5 constructs the kinds")
)]
pub(crate) enum Kind {
    Literal,
    Callee,
    Local,
    Argument,
    Declaration,
}

impl Kind {
    /// The name in `fuzz/known_cascades.txt`: "literal", "callee", "local", "argument",
    /// "declaration".
    #[cfg_attr(not(test), expect(dead_code, reason = "Task 2, Step 5 reads the name"))]
    pub(crate) fn name(self) -> &'static str {
        match self {
            Kind::Literal => "literal",
            Kind::Callee => "callee",
            Kind::Local => "local",
            Kind::Argument => "argument",
            Kind::Declaration => "declaration",
        }
    }

    /// The kind that `name` gives, if any.
    #[cfg_attr(not(test), expect(dead_code, reason = "Task 2, Step 5 reads the name"))]
    pub(crate) fn named(name: &str) -> Option<Kind> {
        [
            Kind::Literal,
            Kind::Callee,
            Kind::Local,
            Kind::Argument,
            Kind::Declaration,
        ]
        .into_iter()
        .find(|kind| kind.name() == name)
    }
}

/// A site as the writer records it, in its piece's `program`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MistakeSite {
    pub(crate) kind: Kind,
    /// The text that the edit replaces: the literal, the callee's name, the local's name, the
    /// removed argument with its `, `, or the declared name.
    pub(crate) at: Range<usize>,
    /// Where the error starts: the call for `Argument`, else `at.start`.
    pub(crate) error: usize,
    /// The index in `Scope::funcs` of the function that holds the site; `None` outside them.
    pub(crate) function: Option<usize>,
}

/// One edit of `Generated::program` and the errors that it must give, and nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Mistake {
    pub(crate) kind: Kind,
    pub(crate) function: Option<usize>,
    pub(crate) at: Range<usize>,
    pub(crate) by: String,
    /// The code and the offset in `program`, before the edit, of each error, in order of offset.
    pub(crate) errors: Vec<(&'static str, usize)>,
}

/// At most one mistake of each kind, in kind order, from `sites` of `program`. The site of a
/// kind is `sites_of_kind[hash.rotate_left(13 * kind as u32) % n]`, where `hash` is the 64-bit
/// FNV-1a hash of `data`.
#[expect(
    unused_variables,
    reason = "Task 1, Step 5 chooses the mistakes from the sites"
)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Task 1, Step 5 calls it at the end of `generate`")
)]
pub(crate) fn choose(sites: &[MistakeSite], program: &str, data: &[u8]) -> Vec<Mistake> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::super::coverage;
    use super::super::generate;
    use super::*;
    use crate::lexer::{self, Token, TokenKind};

    const READS: &str = "fn main() {\n    let v1 = [1]\n    print(v1)\n    print(\"{v1}\")\n}\n";

    /// The site of `kind` over the first `text` of `program`, with its error at `error`.
    fn site(kind: Kind, program: &str, text: &str, error: Option<usize>) -> MistakeSite {
        let start = program
            .find(text)
            .expect("the program should hold the text");
        MistakeSite {
            kind,
            at: start..start + text.len(),
            error: error.unwrap_or(start),
            function: None,
        }
    }

    fn one(kind: Kind, program: &str, text: &str) -> Option<Mistake> {
        let sites = [site(kind, program, text, None)];
        choose(&sites, program, b"").pop()
    }

    #[test]
    fn kind_named_should_find_each_name() {
        let kinds = [
            Kind::Literal,
            Kind::Callee,
            Kind::Local,
            Kind::Argument,
            Kind::Declaration,
        ];
        for kind in kinds {
            assert_eq!(Kind::named(kind.name()), Some(kind));
        }
        assert_eq!(Kind::named("bound"), None);
    }

    #[test]
    fn choose_should_find_each_use_of_a_declared_local() {
        let sites = [site(Kind::Declaration, READS, "v1", None)];
        let mistake = choose(&sites, READS, b"").pop().expect("a mistake");
        let call = READS.find("print(v1)").expect("the call") + "print(".len();
        let part = READS.find("{v1}").expect("the part") + 1;
        assert_eq!(mistake.by, "missing");
        assert_eq!(mistake.errors, [("E0033", call), ("E0033", part)]);
    }

    #[test]
    fn choose_should_drop_a_declared_local_with_no_use() {
        let program = "fn main() {\n    let v1 = [1]\n}\n";
        assert_eq!(one(Kind::Declaration, program, "v1"), None);
    }

    #[test]
    fn choose_should_take_a_name_that_the_program_does_not_hold() {
        let program = "fn main() {\n    let missing = 1\n    f1(missing)\n}\n";
        let mistake = one(Kind::Callee, program, "f1").expect("a mistake");
        assert_eq!(mistake.by, "missing1");
        assert_eq!(mistake.errors, [("E0052", mistake.at.start)]);
    }

    #[test]
    fn choose_should_replace_a_bool_literal_with_a_string() {
        let program = "fn main() {\n    let v1 = true\n    let v2 = 5\n}\n";
        let mistake = one(Kind::Literal, program, "true").expect("a mistake");
        assert_eq!(mistake.by, "\"x\"");
        assert_eq!(mistake.errors, [("E0032", mistake.at.start)]);
        let mistake = one(Kind::Literal, program, "5").expect("a mistake");
        assert_eq!(mistake.by, "true");
        assert_eq!(mistake.errors, [("E0032", mistake.at.start)]);
    }

    #[test]
    fn choose_should_remove_the_argument_and_expect_e0053_at_the_call() {
        let program = "fn main() {\n    f1(0, 1)\n}\n";
        let call = program.find("f1").expect("the call");
        let sites = [site(Kind::Argument, program, ", 1", Some(call))];
        let mistake = choose(&sites, program, b"").pop().expect("a mistake");
        assert_eq!(mistake.by, "");
        assert_eq!(mistake.errors, [("E0053", call)]);
    }

    #[test]
    fn choose_should_take_at_most_one_site_of_each_kind() {
        let program = "fn main() {\n    print(a)\n    print(b)\n    print(c)\n}\n";
        let sites: Vec<MistakeSite> = ["a", "b", "c"]
            .into_iter()
            .map(|name| site(Kind::Local, program, name, None))
            .collect();
        assert_eq!(choose(&sites, program, b"data").len(), 1);
    }

    #[test]
    fn choose_should_rename_a_read_of_self() {
        let program = "fn main() {\n    print(self.g)\n}\n";
        let mistake = one(Kind::Local, program, "self").expect("a mistake");
        assert_eq!(mistake.by, "missing");
        assert_eq!(mistake.errors, [("E0033", mistake.at.start)]);
    }

    fn tokens(program: &str) -> Vec<Token> {
        lexer::lex(program).expect("the program should lex").tokens
    }

    /// Whether `tokens` hold one token of a kind that `kinds` accepts over exactly `at`.
    fn token_over(tokens: &[Token], at: &Range<usize>, kinds: fn(&TokenKind) -> bool) -> bool {
        tokens
            .iter()
            .any(|token| token.span.range() == *at && kinds(&token.kind))
    }

    /// Whether `at` is `(-<number>)`: four tokens that meet.
    fn negative_number(tokens: &[Token], at: &Range<usize>) -> bool {
        tokens.windows(4).any(|w| {
            matches!(
                (&w[0].kind, &w[1].kind, &w[2].kind, &w[3].kind),
                (
                    TokenKind::LParen,
                    TokenKind::Minus,
                    TokenKind::Int(_),
                    TokenKind::RParen
                )
            ) && w[0].span.range().start == at.start
                && w[3].span.range().end == at.end
        })
    }

    fn check_text_of_kind(n: u64) {
        let g = generate(&coverage::seed(n));
        let tokens = tokens(&g.program);
        for m in &g.mistakes {
            let text = &g.program[m.at.clone()];
            let ok = match m.kind {
                Kind::Callee | Kind::Declaration => {
                    token_over(&tokens, &m.at, |k| matches!(k, TokenKind::Ident(_)))
                }
                Kind::Local => token_over(&tokens, &m.at, |k| {
                    matches!(k, TokenKind::Ident(_) | TokenKind::SelfKw)
                }),
                Kind::Literal => {
                    token_over(&tokens, &m.at, |k| {
                        matches!(
                            k,
                            TokenKind::True
                                | TokenKind::False
                                | TokenKind::Int(_)
                                | TokenKind::Char(_)
                                | TokenKind::Str(_)
                        )
                    }) || negative_number(&tokens, &m.at)
                }
                Kind::Argument => {
                    let call = &g.program[m.errors[0].1..];
                    (text.starts_with(", ")
                        || text == "depth - 1"
                        || text.bytes().all(|b| b.is_ascii_digit()))
                        && (call.starts_with("((") || call.starts_with('f'))
                }
            };
            assert!(ok, "seed {n}: {:?} replaces {text:?}", m.kind);
            if m.kind == Kind::Declaration {
                for &(_, at) in &m.errors {
                    let end = at + text.len();
                    let named = tokens.iter().any(|t| {
                        t.span.range() == (at..end)
                            && matches!(t.kind, TokenKind::Ident(_))
                            && g.program[at..end] == *text
                    });
                    assert!(named, "seed {n}: no `{text}` at {at}");
                }
            }
        }
    }

    #[test]
    fn each_kind_should_have_a_site_in_some_seed() {
        let mut kinds = Vec::new();
        for n in 0..coverage::SEEDS {
            let g = generate(&coverage::seed(n));
            kinds.extend(g.mistakes.iter().map(|m| m.kind));
        }
        kinds.sort();
        kinds.dedup();
        assert_eq!(kinds.len(), 5, "kinds found: {kinds:?}");
    }

    #[test]
    fn each_site_should_hold_the_text_of_its_kind() {
        for n in 0..coverage::SEEDS {
            check_text_of_kind(n);
        }
    }
}
