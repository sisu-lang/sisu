//! The join-order check: the members of a join, in the other order. A join of an `if` or of an
//! array literal must not depend on the order of its members. So a program that checks clean
//! must still check clean when they swap. The swap reads tokens, not the syntax tree: the text
//! is a generated program, which the clean check already passed. A swap of an array site must
//! parse, which leaves out the brackets of a type.

use std::ops::Range;

use crate::lexer::{self, Token, TokenKind};
use crate::parser;

/// `program` with the two blocks of each `if` that has an `else` block swapped. It also swaps the
/// first and last elements of each array literal of two or more elements. None when it has no such
/// join, or does not lex. An `if let` keeps its order: its `else` block cannot see the binding.
/// An `else if` swaps only the blocks of its inner `if`.
pub(crate) fn swap_joins(program: &str) -> Option<String> {
    let lexed = lexer::lex(program).ok()?;
    let tokens: Vec<&Token> = (lexed.tokens.iter())
        .filter(|t| t.kind != TokenKind::Newline)
        .collect();
    let mut pairs: Vec<(Range<usize>, Range<usize>)> = (0..tokens.len())
        .filter_map(|i| match tokens[i].kind {
            TokenKind::If => if_blocks(&tokens, i),
            // The commas of type arguments look like those of elements, as in `[Pair<i64, u8>]`.
            // Such a swap does not parse, so the parse keeps only array literals. Only a `<`
            // can start type arguments, and the parse is slow, so it runs only then.
            TokenKind::LBracket => outer_elements(&tokens, i).filter(|pair| {
                let close = closing(&tokens, i, &TokenKind::LBracket, &TokenKind::RBracket);
                let angled = close
                    .is_some_and(|close| tokens[i..close].iter().any(|t| t.kind == TokenKind::Lt));
                !angled
                    || parses(&render(
                        program,
                        0..program.len(),
                        std::slice::from_ref(pair),
                        &mut 0,
                    ))
            }),
            _ => None,
        })
        .collect();
    if pairs.is_empty() {
        return None;
    }
    // An enclosing pair comes before the pairs inside it.
    pairs.sort_by_key(|(a, b)| (a.start, std::cmp::Reverse(b.end)));
    Some(render(program, 0..program.len(), &pairs, &mut 0))
}

/// Whether `source` lexes and parses with no error.
fn parses(source: &str) -> bool {
    lexer::lex(source).is_ok_and(|lexed| {
        let parsed = parser::parse(lexed, false);
        parsed.diagnostics.is_empty() && parsed.checkable
    })
}

/// The then and else blocks of the `if` at `tokens[i]`, `{` through `}`.
fn if_blocks(tokens: &[&Token], i: usize) -> Option<(Range<usize>, Range<usize>)> {
    if tokens.get(i + 1)?.kind == TokenKind::Let {
        return None;
    }
    // The condition holds no `{` outside brackets, so the first one opens the then block. A
    // guard of a match arm reaches its `=>` first, and has no blocks.
    let mut depth = 0_usize;
    let mut open = None;
    for (j, token) in tokens.iter().enumerate().skip(i + 1) {
        match token.kind {
            TokenKind::LBrace if depth == 0 => {
                open = Some(j);
                break;
            }
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::FatArrow if depth == 0 => return None,
            _ => {}
        }
    }
    let open = open?;
    let close = closing(tokens, open, &TokenKind::LBrace, &TokenKind::RBrace)?;
    if tokens.get(close + 1)?.kind != TokenKind::Else
        || tokens.get(close + 2)?.kind != TokenKind::LBrace
    {
        return None;
    }
    let else_close = closing(tokens, close + 2, &TokenKind::LBrace, &TokenKind::RBrace)?;
    Some((
        span(tokens, open, close),
        span(tokens, close + 2, else_close),
    ))
}

/// The first and last elements of the array literal that `tokens[i]` opens, when it has two or
/// more. An index (`xs[i]`), an array type (`[i64]`) and a fill (`[v; n]`) have no such pair.
fn outer_elements(tokens: &[&Token], i: usize) -> Option<(Range<usize>, Range<usize>)> {
    let close = closing(tokens, i, &TokenKind::LBracket, &TokenKind::RBracket)?;
    // The token ranges of the elements, split at the commas that are not inside brackets.
    let mut elements = Vec::new();
    let (mut depth, mut start) = (0_usize, i + 1);
    for (j, token) in tokens.iter().enumerate().take(close).skip(i + 1) {
        match token.kind {
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::Comma if depth == 0 => {
                elements.push(start..j);
                start = j + 1;
            }
            _ => {}
        }
    }
    // A trailing comma leaves no element after it.
    if start < close {
        elements.push(start..close);
    }
    let (first, last) = (elements.first()?, elements.last()?);
    (elements.len() >= 2).then(|| {
        (
            span(tokens, first.start, first.end - 1),
            span(tokens, last.start, last.end - 1),
        )
    })
}

/// The index of the token that closes the bracket `open` at `tokens[at]`.
fn closing(tokens: &[&Token], at: usize, open: &TokenKind, close: &TokenKind) -> Option<usize> {
    let mut depth = 0_usize;
    (at..tokens.len()).find(|&j| {
        if tokens[j].kind == *open {
            depth += 1;
        } else if tokens[j].kind == *close {
            depth -= 1;
        }
        depth == 0
    })
}

/// The source range from the start of `tokens[first]` to the end of `tokens[last]`.
fn span(tokens: &[&Token], first: usize, last: usize) -> Range<usize> {
    tokens[first].span.range().start..tokens[last].span.range().end
}

/// The text of `range` of `program` with each pair from `pairs[*next]` on that starts in it
/// swapped, the pairs inside each part swapped too. `pairs` is in order of start, an enclosing
/// pair first, and two pairs either nest or do not overlap.
fn render(
    program: &str,
    range: Range<usize>,
    pairs: &[(Range<usize>, Range<usize>)],
    next: &mut usize,
) -> String {
    let mut out = String::new();
    let mut at = range.start;
    while let Some((a, b)) = pairs.get(*next).filter(|(a, _)| a.start < range.end) {
        let (a, b) = (a.clone(), b.clone());
        *next += 1;
        out += &program[at..a.start];
        let first = render(program, a.clone(), pairs, next);
        let between = render(program, a.end..b.start, pairs, next);
        let second = render(program, b.clone(), pairs, next);
        out += &second;
        out += &between;
        out += &first;
        at = b.end;
    }
    out += &program[at..range.end];
    out
}

#[cfg(test)]
mod tests {
    use super::swap_joins;

    fn swapped(program: &str) -> String {
        swap_joins(program).unwrap_or_else(|| program.to_string())
    }

    #[test]
    fn swap_joins_should_swap_the_blocks_of_an_if() {
        assert_eq!(
            swapped("let x = if c { 1 } else { None }\n"),
            "let x = if c { None } else { 1 }\n"
        );
    }

    #[test]
    fn swap_joins_should_swap_the_first_and_last_elements_of_an_array() {
        assert_eq!(swapped("f([a, g(b, c), d])\n"), "f([d, g(b, c), a])\n");
    }

    #[test]
    fn swap_joins_should_swap_inside_each_swapped_part() {
        assert_eq!(
            swapped("if c { [1, None] } else { if d { [x, y] } else { z } }\n"),
            "if c { if d { z } else { [y, x] } } else { [None, 1] }\n"
        );
    }

    #[test]
    fn swap_joins_should_swap_only_the_inner_if_of_an_else_if() {
        assert_eq!(
            swapped("if a { 1 } else if b { 2 } else { 3 }\n"),
            "if a { 1 } else if b { 3 } else { 2 }\n"
        );
    }

    #[test]
    fn swap_joins_should_leave_other_forms_as_written() {
        let program =
            "if let Some(v) = o { v } else { 0 }\nlet xs: [i64] = [0; 3]\nxs[0]\nif c { 1 }\n[1]\n";
        assert_eq!(swap_joins(program), None);
    }

    #[test]
    fn swap_joins_should_leave_an_array_type_as_written() {
        let program = "fn f(x: [Pair<i64, u8>]) {}\n";
        assert_eq!(swap_joins(program), None);
    }

    #[test]
    fn swap_joins_should_leave_a_match_guard_as_written() {
        let program = "match n { v if v > 0 => 1, _ => 2 }\n";
        assert_eq!(swap_joins(program), None);
    }

    #[test]
    fn swap_joins_should_read_a_bracket_in_a_condition() {
        assert_eq!(
            swapped("if xs[(i)] == 1 { a } else { b }\n"),
            "if xs[(i)] == 1 { b } else { a }\n"
        );
    }

    #[test]
    fn swap_joins_should_swap_an_array_literal_that_holds_a_less_than() {
        assert_eq!(
            swapped("fn main() {\n    let xs = [1 < 2, false]\n}\n"),
            "fn main() {\n    let xs = [false, 1 < 2]\n}\n"
        );
    }

    #[test]
    fn swap_joins_should_ignore_a_trailing_comma() {
        assert_eq!(swapped("[1, 2,]\n"), "[2, 1,]\n");
    }

    #[test]
    fn swap_joins_should_keep_a_comma_inside_a_call_in_its_element() {
        assert_eq!(swapped("[f(a, b), c]\n"), "[c, f(a, b)]\n");
    }

    #[test]
    fn swap_joins_should_swap_the_fills_of_an_array_literal() {
        assert_eq!(swapped("[[0; 2], [1; 3]]\n"), "[[1; 3], [0; 2]]\n");
    }

    #[test]
    fn swap_joins_should_swap_an_if_whose_condition_is_an_if() {
        assert_eq!(
            swapped("if (if c { true } else { false }) { a } else { b }\n"),
            "if (if c { false } else { true }) { b } else { a }\n"
        );
    }

    #[test]
    fn swap_joins_should_swap_an_if_after_a_match_guard() {
        assert_eq!(
            swapped("match n { v if v > 0 => if c { 1 } else { 2 }, _ => 3 }\n"),
            "match n { v if v > 0 => if c { 2 } else { 1 }, _ => 3 }\n"
        );
    }

    #[test]
    fn swap_joins_should_swap_an_if_whose_condition_is_a_match() {
        assert_eq!(
            swapped("if (match n { _ => true }) { a } else { b }\n"),
            "if (match n { _ => true }) { b } else { a }\n"
        );
    }
}
