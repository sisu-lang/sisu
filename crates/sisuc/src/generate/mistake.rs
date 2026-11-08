//! Property 7, the single-mistake test: the sites where the generator knows the errors of one
//! edit, and the choice of at most one mistake of each kind. The writer records each site while it
//! writes the text (`Writer::record`); `choose` turns the sites of a program into edits.

use std::ops::Range;

use rustc_hash::FxHashSet;

use crate::lexer::{self, TokenKind};

/// The kinds of property 7, in the order of the spec's table. The bound kind is property 6.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Kind {
    Literal,
    Callee,
    Local,
    Argument,
    Declaration,
}

impl Kind {
    /// Every kind, in kind order.
    pub(crate) const ALL: [Kind; 5] = [
        Kind::Literal,
        Kind::Callee,
        Kind::Local,
        Kind::Argument,
        Kind::Declaration,
    ];

    /// The name in `fuzz/known_cascades.txt`: "literal", "callee", "local", "argument",
    /// "declaration".
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
    pub(crate) fn named(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.name() == name)
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
/// FNV-1a hash of `data`. The choice reads no stream of the generator, so no program changes.
pub(crate) fn choose(sites: &[MistakeSite], program: &str, data: &[u8]) -> Vec<Mistake> {
    // A program that does not lex has no mistake: the other properties report it.
    let Ok(lexed) = lexer::lex(program) else {
        return Vec::new();
    };
    // Each identifier token, as its text and where it starts.
    let idents: Vec<(&str, usize)> = (lexed.tokens.iter())
        .filter(|token| matches!(token.kind, TokenKind::Ident(_)))
        .map(|token| {
            let range = token.span.range();
            (&program[range.clone()], range.start)
        })
        .collect();
    // Where each `{` and `}` is, so that a declaration reads to the end of its block.
    let braces: Vec<(usize, bool)> = (lexed.tokens.iter())
        .filter_map(|token| match token.kind {
            TokenKind::LBrace => Some((token.span.range().start, true)),
            TokenKind::RBrace => Some((token.span.range().start, false)),
            _ => None,
        })
        .collect();
    // The spellings, so that the search for a fresh name probes once for each candidate.
    let spellings: FxHashSet<&str> = idents.iter().map(|&(ident, _)| ident).collect();
    let hash = fnv1a(data);
    Kind::ALL
        .into_iter()
        .filter_map(|kind| {
            let of_kind: Vec<&MistakeSite> = sites.iter().filter(|s| s.kind == kind).collect();
            let count = u64::try_from(of_kind.len()).ok().filter(|&n| n > 0)?;
            let pick = hash.rotate_left(13 * kind as u32) % count;
            let site = of_kind[usize::try_from(pick).ok()?];
            edit(site, program, &idents, &braces, &spellings)
        })
        .collect()
}

/// The edit of `site`, and the errors that it gives, or `None` when the edit gives none.
fn edit(
    site: &MistakeSite,
    program: &str,
    idents: &[(&str, usize)],
    braces: &[(usize, bool)],
    spellings: &FxHashSet<&str>,
) -> Option<Mistake> {
    let text = &program[site.at.clone()];
    let fresh = || {
        let mut name = "missing".to_string();
        let mut n = 0;
        while spellings.contains(name.as_str()) {
            n += 1;
            name = format!("missing{n}");
        }
        name
    };
    let (by, errors) = match site.kind {
        Kind::Literal => {
            let by = if matches!(text, "true" | "false") {
                "\"x\""
            } else {
                "true"
            };
            (by.to_string(), vec![("E0032", site.at.start)])
        }
        Kind::Callee => (fresh(), vec![("E0052", site.at.start)]),
        Kind::Local => (fresh(), vec![("E0033", site.at.start)]),
        Kind::Argument => (String::new(), vec![("E0053", site.error)]),
        Kind::Declaration => {
            // Each read of the local in its block is an error; with none, the edit gives none.
            // Another function may reuse the name (`nest` declares `v0`, `v1`, ...).
            let end = block_end(braces, site.at.end);
            let uses: Vec<_> = (idents.iter())
                .filter(|&&(ident, start)| ident == text && (site.at.end..end).contains(&start))
                .map(|&(_, start)| ("E0033", start))
                .collect();
            if uses.is_empty() {
                return None;
            }
            (fresh(), uses)
        }
    };
    Some(Mistake {
        kind: site.kind,
        function: site.function,
        at: site.at.clone(),
        by,
        errors,
    })
}

/// The offset of the `}` that closes the block which holds `from`, or `usize::MAX` when none does.
fn block_end(braces: &[(usize, bool)], from: usize) -> usize {
    let mut depth = 0_usize;
    for &(at, open) in braces.iter().filter(|&&(at, _)| at >= from) {
        if open {
            depth += 1;
        } else if let Some(inner) = depth.checked_sub(1) {
            depth = inner;
        } else {
            return at;
        }
    }
    usize::MAX
}

/// The 64-bit FNV-1a hash of `data`.
fn fnv1a(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::super::coverage;
    use super::super::generate;
    use super::*;
    use crate::lexer::Token;

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
        for kind in Kind::ALL {
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

    /// GIVEN two functions that each declare and read `v1`, as `main` and the fixed `nest` do
    /// WHEN the declaration of the first one is edited
    /// THEN the errors are the reads in its own block, found in a nested block too, and not the
    /// reads of the other function
    #[test]
    fn choose_should_find_the_uses_of_a_declared_local_in_its_block_only() {
        let program = "fn f1() {\n    let v1 = [1]\n    if true {\n        print(v1)\n    }\n}\nfn main() {\n    let v1 = [2]\n    print(v1)\n}\n";
        let mistake = one(Kind::Declaration, program, "v1").expect("a mistake");
        let read = program.find("print(v1)").expect("the read") + "print(".len();
        assert_eq!(mistake.errors, [("E0033", read)]);
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
        let program = "fn main() {\n    print(v1)\n    print(v2)\n    print(v3)\n}\n";
        let sites: Vec<MistakeSite> = ["v1", "v2", "v3"]
            .into_iter()
            .map(|name| site(Kind::Local, program, name, None))
            .collect();
        assert_eq!(choose(&sites, program, b"data").len(), 1);
    }

    /// The hash of `ab` is `0x089c4407b545986a`. The site counts are 4, 4, 5, 5 and 4. With a
    /// rotation of 13 bits for each kind, the hash picks the sites 2, 3, 3, 1 and 1.
    /// Other choices pick other sites. A hash of 0 picks 0 for each kind. Without the rotation,
    /// the hash picks 2 for each kind. The sites in reverse order give 1, 0, 1, 3 and 2.
    #[test]
    fn choose_should_pick_each_site_by_the_hash_of_the_data() {
        let counts = [4, 4, 5, 5, 4];
        let picks = [2, 3, 3, 1, 1];
        let mut program = String::from("fn main() {\n");
        let mut sites = Vec::new();
        for (kind, count) in Kind::ALL.into_iter().zip(counts) {
            for n in 0..count {
                let name = format!("{}{n}", kind.name());
                let start = program.len() + "    ".len();
                let line = format!("    {name}\n");
                program += &line;
                if kind == Kind::Declaration {
                    // A read of the local, so that the edit gives an error.
                    program += &line;
                }
                sites.push(MistakeSite {
                    kind,
                    at: start..start + name.len(),
                    error: start,
                    function: None,
                });
            }
        }
        program += "}\n";
        let mistakes = choose(&sites, &program, b"ab");
        let taken: Vec<&str> = mistakes.iter().map(|m| &program[m.at.clone()]).collect();
        let wanted: Vec<String> = Kind::ALL
            .into_iter()
            .zip(picks)
            .map(|(kind, pick)| format!("{}{pick}", kind.name()))
            .collect();
        assert_eq!(taken, wanted);
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
                    // The call starts with `((` of a method, or with `f<n>`; the removed text
                    // ends the arguments.
                    let call = &g.program[m.errors[0].1..];
                    let callee = call
                        .strip_prefix('f')
                        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
                    (text.starts_with(", ")
                        || text == "depth - 1"
                        || text.bytes().all(|b| b.is_ascii_digit()))
                        && !text.is_empty()
                        && g.program[m.at.end..].starts_with(')')
                        && (call.starts_with("((") || callee)
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
