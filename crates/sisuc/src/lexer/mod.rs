//! The lexer: source text to tokens.

mod literal;

use std::fmt::Write;
use std::iter::Peekable;
use std::ops::Range;
use std::str::CharIndices;

use crate::diagnostic::{Code, Diagnostic, DiagnosticKind, Join};
use crate::source::{LineIndex, Span};
use crate::symbol::Symbol;
use literal::End;
pub(crate) use literal::Float;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TokenKind {
    Ident(Symbol),
    /// A lone `_`, which only `let` and `for` accept (`_x` and `x_` are names).
    Underscore,
    /// An integer literal: its magnitude, at most `MIN_MAGNITUDE`, which only a unary `-`
    /// accepts. Its text as written is `source[token.span.range()]`.
    Int(u64),
    /// A float literal: its value, rounded to the nearest `f64`.
    Float(Float),
    /// A `char` literal, decoded.
    Char(char),
    /// A string literal without interpolation, decoded.
    Str(String),
    /// The text of a string literal up to its first interpolation, decoded. The interpolation's
    /// tokens follow, then a `StrMiddle` before each further interpolation and a `StrTail`.
    StrHead(String),
    /// The text between two interpolations, decoded.
    StrMiddle(String),
    /// The text after the last interpolation, decoded.
    StrTail(String),
    /// The text of a format spec, after its `:`; the token's span starts at the `:`.
    FormatSpec(String),
    Fn,
    Let,
    Mut,
    Var,
    /// A word that a later milestone gives meaning. The milestone that does so gives it its own
    /// variant and removes it from `keyword_or_ident`.
    Reserved(&'static str),
    Todo,
    If,
    Else,
    While,
    For,
    Loop,
    In,
    Return,
    Break,
    Continue,
    Class,
    Const,
    Enum,
    Match,
    NoneKw,
    Is,
    SelfKw,
    True,
    False,
    Plus,
    Minus,
    Star,
    StarStar,
    Slash,
    Percent,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
    Bang,
    Eq,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Arrow,
    FatArrow,
    Pipe,
    Dot,
    DotDot,
    DotDotEq,
    LBracket,
    RBracket,
    Semicolon,
    Question,
    QuestionDot,
    QuestionQuestion,
    Newline,
    Eof,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Token {
    pub(crate) kind: TokenKind,
    pub(crate) span: Span,
}

// This size holds only on 64-bit targets: `TokenKind` holds `String`s, sized by the pointer width.
#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<Token>() == 40);

impl TokenKind {
    /// The variant name, as `--emit tokens` prints it.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            TokenKind::Ident(_) => "Ident",
            TokenKind::Underscore => "Underscore",
            TokenKind::Int(_) => "Int",
            TokenKind::Float(_) => "Float",
            TokenKind::Char(_) => "Char",
            TokenKind::Str(_) => "Str",
            TokenKind::StrHead(_) => "StrHead",
            TokenKind::StrMiddle(_) => "StrMiddle",
            TokenKind::StrTail(_) => "StrTail",
            TokenKind::FormatSpec(_) => "FormatSpec",
            TokenKind::Fn => "Fn",
            TokenKind::Let => "Let",
            TokenKind::Mut => "Mut",
            TokenKind::Var => "Var",
            TokenKind::Reserved(_) => "Reserved",
            TokenKind::Todo => "Todo",
            TokenKind::If => "If",
            TokenKind::Else => "Else",
            TokenKind::While => "While",
            TokenKind::For => "For",
            TokenKind::Loop => "Loop",
            TokenKind::In => "In",
            TokenKind::Return => "Return",
            TokenKind::Break => "Break",
            TokenKind::Continue => "Continue",
            TokenKind::Class => "Class",
            TokenKind::Const => "Const",
            TokenKind::Enum => "Enum",
            TokenKind::Match => "Match",
            TokenKind::NoneKw => "NoneKw",
            TokenKind::Is => "Is",
            TokenKind::SelfKw => "SelfKw",
            TokenKind::True => "True",
            TokenKind::False => "False",
            TokenKind::Plus => "Plus",
            TokenKind::Minus => "Minus",
            TokenKind::Star => "Star",
            TokenKind::StarStar => "StarStar",
            TokenKind::Slash => "Slash",
            TokenKind::Percent => "Percent",
            TokenKind::PlusEq => "PlusEq",
            TokenKind::MinusEq => "MinusEq",
            TokenKind::StarEq => "StarEq",
            TokenKind::SlashEq => "SlashEq",
            TokenKind::PercentEq => "PercentEq",
            TokenKind::EqEq => "EqEq",
            TokenKind::NotEq => "NotEq",
            TokenKind::Lt => "Lt",
            TokenKind::Le => "Le",
            TokenKind::Gt => "Gt",
            TokenKind::Ge => "Ge",
            TokenKind::AndAnd => "AndAnd",
            TokenKind::OrOr => "OrOr",
            TokenKind::Bang => "Bang",
            TokenKind::Eq => "Eq",
            TokenKind::LParen => "LParen",
            TokenKind::RParen => "RParen",
            TokenKind::LBrace => "LBrace",
            TokenKind::RBrace => "RBrace",
            TokenKind::Comma => "Comma",
            TokenKind::Colon => "Colon",
            TokenKind::Arrow => "Arrow",
            TokenKind::FatArrow => "FatArrow",
            TokenKind::Pipe => "Pipe",
            TokenKind::Dot => "Dot",
            TokenKind::DotDot => "DotDot",
            TokenKind::DotDotEq => "DotDotEq",
            TokenKind::LBracket => "LBracket",
            TokenKind::RBracket => "RBracket",
            TokenKind::Semicolon => "Semicolon",
            TokenKind::Question => "Question",
            TokenKind::QuestionDot => "QuestionDot",
            TokenKind::QuestionQuestion => "QuestionQuestion",
            TokenKind::Newline => "Newline",
            TokenKind::Eof => "Eof",
        }
    }
}

impl TokenKind {
    /// How a diagnostic names the token: "`)`", "`x`", "end of line".
    pub(crate) fn describe(&self) -> String {
        let text = match self {
            TokenKind::Ident(name) => name.as_str(),
            TokenKind::Underscore => "_",
            TokenKind::Int(_) => return "an integer literal".to_string(),
            TokenKind::Float(x) => return format!("`{x}`"),
            TokenKind::Char(c) => return format!("`{c:?}`"),
            TokenKind::Str(_) | TokenKind::StrHead(_) => return "a string literal".to_string(),
            TokenKind::Fn => "fn",
            TokenKind::Let => "let",
            TokenKind::Mut => "mut",
            TokenKind::Var => "var",
            TokenKind::Reserved(word) => word,
            TokenKind::Todo => "todo",
            TokenKind::If => "if",
            TokenKind::Else => "else",
            TokenKind::While => "while",
            TokenKind::For => "for",
            TokenKind::Loop => "loop",
            TokenKind::In => "in",
            TokenKind::Return => "return",
            TokenKind::Break => "break",
            TokenKind::Continue => "continue",
            TokenKind::Class => "class",
            TokenKind::Const => "const",
            TokenKind::Enum => "enum",
            TokenKind::Match => "match",
            TokenKind::NoneKw => "None",
            TokenKind::Is => "is",
            TokenKind::SelfKw => "self",
            TokenKind::True => "true",
            TokenKind::False => "false",
            TokenKind::Plus => "+",
            TokenKind::Minus => "-",
            TokenKind::Star => "*",
            TokenKind::StarStar => "**",
            TokenKind::Slash => "/",
            TokenKind::Percent => "%",
            TokenKind::PlusEq => "+=",
            TokenKind::MinusEq => "-=",
            TokenKind::StarEq => "*=",
            TokenKind::SlashEq => "/=",
            TokenKind::PercentEq => "%=",
            TokenKind::EqEq => "==",
            TokenKind::NotEq => "!=",
            TokenKind::Lt => "<",
            TokenKind::Le => "<=",
            TokenKind::Gt => ">",
            TokenKind::Ge => ">=",
            TokenKind::AndAnd => "&&",
            TokenKind::OrOr => "||",
            TokenKind::Bang => "!",
            TokenKind::Eq => "=",
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::LBrace => "{",
            // A string piece after an interpolation starts at the `}` that ends it.
            TokenKind::RBrace | TokenKind::StrMiddle(_) | TokenKind::StrTail(_) => "}",
            TokenKind::Comma => ",",
            // A format spec starts at its `:`.
            TokenKind::Colon | TokenKind::FormatSpec(_) => ":",
            TokenKind::Arrow => "->",
            TokenKind::FatArrow => "=>",
            TokenKind::Pipe => "|",
            TokenKind::Dot => ".",
            TokenKind::DotDot => "..",
            TokenKind::DotDotEq => "..=",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::Semicolon => ";",
            TokenKind::Question => "?",
            TokenKind::QuestionDot => "?.",
            TokenKind::QuestionQuestion => "??",
            TokenKind::Newline => return "end of line".to_string(),
            TokenKind::Eof => return "end of file".to_string(),
        };
        format!("`{text}`")
    }
}

type Chars<'a> = Peekable<CharIndices<'a>>;

/// An open bracket: its char (`(`, `[` or `{`) and where it stands.
type Open = (char, Span);

/// What the lexer makes of a source: the source, whose text the token spans index, its tokens,
/// which always end with `Eof`, a diagnostic for each error it recovered from, a warning for each
/// `;` it dropped, where each `//` comment starts, and where each line break is. Both lists are
/// in source order.
#[derive(Debug)]
pub(crate) struct Lexed<'src> {
    pub(crate) source: &'src str,
    pub(crate) tokens: Vec<Token>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) warnings: Vec<Diagnostic>,
    pub(crate) comments: Vec<usize>,
    pub(crate) breaks: Vec<usize>,
}

/// The tokens of `source`, or its first diagnostic: for tests that need source that lexes clean.
#[cfg(test)]
pub(crate) fn lex_clean(source: &str) -> Result<Vec<Token>, Diagnostic> {
    let lexed = lex(source)?;
    match lexed.diagnostics.into_iter().next() {
        Some(d) => Err(d),
        None => Ok(lexed.tokens),
    }
}

/// Lexes `source`, reporting every error it can continue after. `Err` holds the one diagnostic a
/// bracket error leaves: the first recoverable diagnostic, or the bracket error when there is none.
/// A bracket error after a literal that is not closed does not stop the lexer, because the
/// literal ate the closers: a closer pops down to the opener it matches, past brackets opened
/// before the literal only, a stray closer is dropped, and a bracket still open at the end of the
/// file is left to the parser.
/// A `;` that separates statements becomes a `Newline`, and a redundant one is dropped with a
/// warning (`separate`).
pub(crate) fn lex(source: &str) -> Result<Lexed<'_>, Diagnostic> {
    let (mut tokens, mut diagnostics) = (Vec::new(), Vec::new());
    let (mut comments, mut breaks) = (Vec::new(), Vec::new());
    let mut brackets = Brackets::default();
    let mut chars = source.char_indices().peekable();
    // `goes_on` is where the code starts that continues the line before the gap in front of it.
    let (mut first_on_line, mut goes_on) = (true, 0);
    // Where the last skipped text starts.
    let mut skipped = None;
    while let Some((start, c)) = chars.next() {
        let kind = match c {
            ' ' | '\t' | '\r' => continue,
            '\n' => {
                breaks.push(start);
                let cut = brackets.end_line(start, false, &mut tokens, &mut diagnostics);
                skipped = cut.or(skipped);
                first_on_line = true;
                // Look ahead only where a `Newline` would be pushed: the next newlines of a
                // blank run then find `Newline` last and skip it, so no gap is scanned twice.
                if start >= goes_on && ends_statement(&tokens, &mut brackets) {
                    if let Some(gap) = continues_line(&source[start..], &tokens, &mut brackets) {
                        // The line breaks in the gap are not judged again.
                        goes_on = start + gap;
                    } else {
                        // Empty, at the line end: a span over `\n` renders as a two-line range,
                        // and one at `\n` after `\r` sits a column too far right.
                        let end = start - usize::from(source[..start].ends_with('\r'));
                        tokens.push(newline(Span::new(end, end)));
                        brackets.count(false);
                    }
                }
                continue;
            }
            '/' if chars.next_if(|&(_, n)| n == '/').is_some() => {
                let cut = brackets.comment(start, &mut tokens, &mut diagnostics);
                skipped = cut.or(skipped);
                comments.push(start);
                while chars.next_if(|&(_, n)| n != '\n').is_some() {}
                continue;
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let end = take_while(&mut chars, source, |c| {
                    c.is_ascii_alphanumeric() || c == '_'
                });
                keyword_or_ident(&source[start..end])
            }
            c if c.is_ascii_digit() || c == '\'' => {
                literal::token(source, start, &mut chars, &mut diagnostics, &tokens)
            }
            // A `}` that closes no bracket opened in an interpolation ends it, and the literal's
            // text goes on after it. A `:` that ends its expression starts a format spec.
            '"' | '}' | ':' if c == '"' || brackets.at_end() => {
                let (kind, cut) =
                    brackets.string(source, start, &mut chars, &mut tokens, &mut diagnostics);
                // The rest of the line is lost in the literal, as in a skip.
                skipped = cut.or(skipped);
                let Some(kind) = kind else { continue };
                kind
            }
            '-' if matches!(chars.peek(), Some(&(_, ' ' | '\t')))
                && before_semicolons(&tokens, brackets.trailing_semicolons)
                    .is_some_and(|t| t.kind == TokenKind::Newline) =>
            {
                let at = Span::new(start, start + 1);
                if !after_skip(source, &tokens, &brackets, skipped) {
                    diagnostics.push(DiagnosticKind::LeadingBinaryMinus { at }.into());
                }
                TokenKind::Minus
            }
            _ => {
                if let Some(kind) = symbol(c, &mut chars) {
                    kind
                } else {
                    diagnostics.push(unexpected(c, start));
                    skipped = Some(start);
                    continue;
                }
            }
        };
        let span = Span::new(start, chars.peek().map_or(source.len(), |&(i, _)| i));
        // A `;` that separates statements does not end the line's start: the token after it is
        // the first one the source would have there without it.
        let separator = kind == TokenKind::Semicolon && brackets.in_block();
        if first_on_line {
            diagnostics.extend(brackets.line_start(source, &mut tokens, (&kind, span), skipped));
            first_on_line = separator;
        }
        match track_closer(&mut brackets, &kind, span, &diagnostics, &mut tokens) {
            Err(d) => return Err(first_or(diagnostics, d)),
            Ok(false) => continue,
            Ok(true) => {}
        }
        brackets.note(&kind, start, separator);
        tokens.push(Token { kind, span });
    }
    brackets.end_line(source.len(), true, &mut tokens, &mut diagnostics);
    if let Some((bracket, at)) = unclosed_at_end(&brackets.open, &diagnostics) {
        let unclosed = DiagnosticKind::UnclosedBracket { bracket, at };
        return Err(first_or(diagnostics, unclosed.into()));
    }
    if ends_statement(&tokens, &mut brackets) {
        tokens.push(newline(Span::new(source.len(), source.len())));
    }
    let warnings = separate(source, &mut tokens, &brackets.separators);
    tokens.push(eof(&tokens));
    Ok(Lexed {
        source,
        tokens,
        diagnostics,
        warnings,
        comments,
        breaks,
    })
}

/// What becomes of the first `;` of a run.
enum Judgement {
    Separator,
    Redundant,
    Kept,
}

/// Judges each `;` in `separators`, those that `lex` found in statement context: a separator
/// becomes a `Newline` with the `;`'s span, a redundant one is dropped with a W0007, and any
/// other stays for the parser to report. Removing a redundant `;` leaves tokens that the parser
/// reads the same, so its fix is safe. Without a `;` in a block, there is nothing to judge.
fn separate(source: &str, tokens: &mut Vec<Token>, separators: &[Separator]) -> Vec<Diagnostic> {
    let mut warnings = Vec::new();
    if separators.is_empty() {
        return warnings;
    }
    let mut statements = Statements::default();
    let mut indices = 0..tokens.len();
    while let Some(i) = indices.next() {
        let start = tokens[i].span.start();
        let found = separators.binary_search_by_key(&start, |s| s.at);
        let (TokenKind::Semicolon, Ok(found)) = (&tokens[i].kind, found) else {
            continue;
        };
        let end = i + tokens[i..]
            .iter()
            .take_while(|t| t.kind == TokenKind::Semicolon)
            .count();
        // Every `;` after the first is redundant: it leaves the first its neighbours.
        let between_items = separators[found].between_items;
        let redundant = match judge(source, tokens, i..end, between_items, &mut statements) {
            Judgement::Separator => {
                tokens[i].kind = TokenKind::Newline;
                i + 1
            }
            Judgement::Redundant => i,
            Judgement::Kept => i + 1,
        };
        // Only blanks before the run: removing a `;` there must not widen the indentation.
        let line_start = source.as_bytes()[..start]
            .iter()
            .rev()
            .take_while(|&&b| b != b'\n')
            .all(|&b| b == b' ' || b == b'\t');
        warnings.extend(
            (tokens[redundant..end].iter().enumerate())
                .map(|(k, t)| redundant_semicolon(source, t.span, k == 0, line_start)),
        );
        indices.start = end;
    }
    let mut dropped = warnings.iter().map(Diagnostic::span).peekable();
    tokens.retain(|t| dropped.next_if_eq(&t.span).is_none());
    warnings
}

/// What becomes of the first `;` of `run`, a run of `;`s among `tokens`, in statement context.
/// `between_items` is true when the run stands between top-level items, after a `fn`, `class` or
/// `enum`. `statements` reads `tokens` forward, to tell whether a `>` before the run closes a type
/// annotation, which ends a statement as at a line's end.
/// The rules, in order: a `.` after the run is a continuation unless the `;` is needed to end the
/// statement; a `;` that joins two lines into a different statement stays; a `;` next to a line
/// break, a `{`, a `}`, an `else` or an item that needs no separator is redundant.
fn judge(
    source: &str,
    tokens: &[Token],
    run: Range<usize>,
    between_items: bool,
    statements: &mut Statements,
) -> Judgement {
    use TokenKind::{Class, Const, Dot, Else, Enum, Fn, Gt, LBrace, Newline, QuestionDot, RBrace};
    let (i, end) = (run.start, run.end);
    let (prev, next) = (i.checked_sub(1).map(|p| &tokens[p]), tokens.get(end));
    let first = tokens[i].span;
    let break_before = prev
        .is_none_or(|p| p.kind == Newline || source[p.span.end()..first.start()].contains('\n'));
    // A `{` that moved up may have popped the `Newline` after the run, so the source
    // between the run and `next` tells.
    let break_after = next
        .is_none_or(|n| n.kind == Newline || source[first.start()..n.span.start()].contains('\n'));
    let next = next.map(|n| &n.kind);
    // A `>` that closes a type annotation ends a statement, as at a line's end.
    let ends = can_end_statement(prev)
        || prev.is_some_and(|p| p.kind == Gt) && statements.annotates_type(&tokens[..i]);
    if matches!(next, Some(Dot | QuestionDot)) {
        // Without the `;`, the line break before the `.` would continue the line.
        if ends && !break_before {
            Judgement::Separator
        } else {
            Judgement::Kept
        }
    } else if prev.is_some_and(|p| !ends && !matches!(p.kind, LBrace | Newline)) && !break_before {
        // Without the `;`, the lines would join into a different statement.
        Judgement::Kept
    } else if next == Some(&Else)
        || break_before
        || break_after
        || prev.is_none_or(|p| p.kind == LBrace)
        || next == Some(&RBrace)
        || (between_items && matches!(next, Some(Fn | Class | Enum | Const)))
    {
        Judgement::Redundant
    } else if between_items && prev.is_some_and(|p| p.kind != RBrace) && next == Some(&LBrace) {
        // The `;` cuts an item's header, and no item starts with `{`: as a separator,
        // the `;` would make the `{` a second error, after the one at the `;`
        // (`fn main(); { }`). After a `const` or an item's `}`, it separates.
        Judgement::Kept
    } else {
        Judgement::Separator
    }
}

/// The W0007 of the redundant `;` at `at`, the `first` of its run to be removed. Its fix leaves
/// a space when the `;`s it touches, with it, touch code on each side, so that removing them all
/// joins no two tokens. Of `;`s that touch, only the first removed leaves the space, so removing
/// them all leaves one. When `line_start`, only blanks come before its run on the line. Then each
/// `;` of the run also takes the blanks after it: each owns the blanks that follow it, so the edits
/// never overlap, and the line keeps its indentation, whichever fixes apply.
fn redundant_semicolon(source: &str, at: Span, first: bool, line_start: bool) -> Diagnostic {
    let solid = |b: Option<&u8>| b.is_some_and(|b| !b.is_ascii_whitespace());
    let bytes = source.as_bytes();
    let before = at.start().checked_sub(1).and_then(|b| bytes.get(b));
    // Every `;` after a removed one in its run is removed too. The scan runs last, so only the
    // first `;` of touching ones makes it, which keeps a run of n `;`s linear, not quadratic.
    let space = (first || before != Some(&b';'))
        && solid(before)
        && solid(bytes[at.end()..].iter().find(|&&b| b != b';'));
    let blanks = if line_start {
        (bytes[at.end()..].iter())
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count()
    } else {
        0
    };
    let remove = Span::new(at.start(), at.end() + blanks);
    DiagnosticKind::RedundantSemicolon { at, remove, space }.into()
}

/// An open interpolation of a string literal.
struct Frame {
    /// Where its `{` stands among the open brackets, so that no bracket opened in it outlives it.
    brace: usize,
    /// Where its literal's `"` stands.
    quote: usize,
    /// Where its literal's first token stands among the tokens.
    token: usize,
    /// How many diagnostics came before its literal.
    before: usize,
}

/// The open brackets, and the open interpolations of string literals among them, innermost
/// last. An interpolation's `{` is a bracket, so a bracket opened in it closes before it.
#[derive(Default)]
struct Brackets {
    open: Vec<Open>,
    /// Where the latest `LiteralNotClosed` among the diagnostics starts, of those
    /// `track_closer` has read.
    literal: Option<usize>,
    /// How many diagnostics `track_closer` has read.
    read: usize,
    frames: Vec<Frame>,
    /// How many `;`s that separate statements end the tokens, as `in_block` says. A cut that
    /// shortens the tokens resets it.
    trailing_semicolons: usize,
    /// The `;`s that separate statements among the tokens, in source order. A cut leaves
    /// those in the literal it ends: no token starts there, so `separate` never finds them.
    separators: Vec<Separator>,
    /// Whether the top-level item so far is a `fn`, `class` or `enum`, which ends at its `}` and
    /// needs no separator before the next item, rather than a `const` or none.
    // shortcut: assumes no `fn` inside a const's value; revisit with function types or closures
    self_ending: bool,
    /// The statements the tokens end in, read forward.
    statements: Statements,
}

/// A `;` that separates statements, which `separate` judges.
struct Separator {
    /// Where the `;` starts.
    at: usize,
    /// Whether it stands between top-level items, after a `fn`, `class` or `enum`.
    between_items: bool,
}

/// The statements that the tokens read so far end in: one for each open bracket, innermost
/// last, so a block that closes gives back the statement around it as it was. A `>` at a line's
/// end, a `{` that starts a line, and a line break before a name, `,` or `>` ask about theirs,
/// and only the tokens since the last question are read, so none scans back to its statement's
/// start.
///
/// Only tokens up to the last code token are read. Every later change to the tokens is after
/// it: a line-start `{` removes the `Newline` before it, and a cut ends an interpolation, which
/// no line break or line start falls in.
#[derive(Default)]
struct Statements {
    read: usize,
    enclosing: Vec<Statement>,
    current: Statement,
}

/// What a statement's tokens so far, outside its closed brackets, show.
#[derive(Clone, Copy, Default)]
struct Statement {
    /// Whether its first token is `fn`, `class` or `enum`; `None` before its first token.
    header: Option<bool>,
    /// After its last `:` or `->` that no `=` follows, how many more `>` than `<` there are;
    /// `None` when there is no such `:` or `->`.
    annotation: Option<isize>,
    /// How many tokens it has read so far, so the reader can tell the third.
    tokens: usize,
    /// Whether a header's type-parameter list is open. A `<` that is the third token, after
    /// `fn`, `class` or `enum` and a name, opens it. Its names and `,`s keep it open, and its
    /// `>` or any other token closes it. A bracket closes it before it is saved, so a statement
    /// given back at a `}` never has it.
    params: bool,
}

impl Statements {
    /// The innermost statement after reading the tokens before `end`.
    fn read(&mut self, tokens: &[Token], end: usize) -> Statement {
        use TokenKind::{
            Arrow, Colon, Comma, Eq, Ge, Gt, Ident, LBrace, LBracket, LParen, Loop, Lt, Newline,
            RBrace, RBracket, RParen, Semicolon, StrHead, StrTail,
        };
        for (i, t) in tokens.iter().enumerate().take(end).skip(self.read) {
            #[cfg(test)]
            JUDGED.set(JUDGED.get() + 1);
            let s = &mut self.current;
            s.header = s.header.or(Some(matches!(
                t.kind,
                TokenKind::Fn | TokenKind::Class | TokenKind::Enum
            )));
            s.tokens += 1;
            s.params = s.params && matches!(t.kind, Ident(_) | Comma);
            match t.kind {
                // A block's statements start at its `{`. Inside `( )`, `[ ]` or an interpolation
                // the statement around it goes on, though no type annotation does.
                LBrace | LParen | LBracket | StrHead(_) => {
                    let header = if t.kind == LBrace { None } else { s.header };
                    self.enclosing.push(*s);
                    self.current = Statement {
                        header,
                        ..Statement::default()
                    };
                }
                RBrace | RParen | RBracket | StrTail(_) => {
                    self.current = self.enclosing.pop().unwrap_or_default();
                }
                Newline | Semicolon => *s = Statement::default(),
                Colon | Arrow => s.annotation = Some(0),
                // A `>=` after a type annotation closes it and starts its value. So does a loop
                // keyword: it follows a loop label's `:`, which is no annotation.
                Eq | Ge | TokenKind::While | TokenKind::For | Loop => s.annotation = None,
                Gt => s.annotation = s.annotation.map(|a| a + 1),
                Lt => {
                    s.annotation = s.annotation.map(|a| a - 1);
                    s.params = s.tokens == 3
                        && s.header == Some(true)
                        && matches!(tokens[i - 1].kind, Ident(_));
                }
                _ => {}
            }
        }
        self.read = end;
        self.current
    }

    /// Whether the `>` that ends `tokens`, at a line's end, closes a type annotation: in its
    /// statement, a `:` or `->` has no `=` after it, and every `<` after that is closed
    /// (`b: Box<i64>`, `fn f() -> Box<i64>`). Anywhere else the `>` compares (`a >`), closes a
    /// header's type parameters before its `(` (`fn f<T>`), or an inner list of a type that
    /// goes on (`b: Pair<Box<i64>`), and the line goes on. This is the one place that decides
    /// it: every later reader sees a `Newline` or none.
    #[inline(never)]
    fn annotates_type(&mut self, tokens: &[Token]) -> bool {
        self.read(tokens, tokens.len()).annotation == Some(0)
    }

    /// Whether the statement that `tokens` ends with, before the `Newline`s and `;`s after it,
    /// is a `fn`, `class` or `enum` header, where a `>` can only close type arguments, never
    /// compare.
    fn closes_header(&mut self, tokens: &[Token]) -> bool {
        let end = tokens
            .iter()
            .rposition(|t| !matches!(t.kind, TokenKind::Newline | TokenKind::Semicolon))
            .map_or(0, |i| i + 1);
        self.read(tokens, end).header == Some(true)
    }
}

impl Brackets {
    /// Whether the innermost open bracket is a block's `{`, or there is none: a `;` here
    /// separates statements. An interpolation's `{` is not a block.
    fn in_block(&self) -> bool {
        // Without `at_end`, a `;` directly in an interpolation would be a separator.
        self.open.last().is_none_or(|&(c, _)| c == '{') && !self.at_end()
    }

    /// Whether a `}` ends the innermost interpolation: every bracket opened in it is closed.
    fn at_end(&self) -> bool {
        self.frames
            .last()
            .is_some_and(|f| f.brace + 1 == self.open.len())
    }

    /// Counts a token just pushed: a `;` that separates statements, or another token.
    fn count(&mut self, separator: bool) {
        self.trailing_semicolons = if separator {
            self.trailing_semicolons + 1
        } else {
            0
        };
    }

    /// Counts a token of `kind` that starts at `at`, about to be pushed, and notes it when it is a
    /// `;` that `separator` says separates statements.
    fn note(&mut self, kind: &TokenKind, at: usize, separator: bool) {
        match kind {
            TokenKind::Fn | TokenKind::Class | TokenKind::Enum if self.open.is_empty() => {
                self.self_ending = true;
            }
            TokenKind::Const if self.open.is_empty() => self.self_ending = false,
            _ => {}
        }
        if separator {
            let between_items = self.open.is_empty() && self.self_ending;
            self.separators.push(Separator { at, between_items });
        }
        self.count(separator);
    }

    /// The layout error of the token `at`, which starts a line, unless text was `skipped` on the
    /// line above it. A `{` on its own line lexes as if written on the line above, so it pops the
    /// `Newline` before it. No other line-start error follows a `Newline`: each fires only inside
    /// `(` or `[`. That `Newline` sits before the line's `;`s, or is the last token when they are
    /// before it.
    fn line_start(
        &mut self,
        source: &str,
        tokens: &mut Vec<Token>,
        (kind, span): (&TokenKind, Span),
        skipped: Option<usize>,
    ) -> Option<Diagnostic> {
        let d = check_line_start(source, tokens, &self.open, &mut self.statements, kind, span)?;
        let at = tokens.len().checked_sub(self.trailing_semicolons + 1);
        if let Some(at) =
            at.filter(|&i| tokens.get(i).is_some_and(|t| t.kind == TokenKind::Newline))
        {
            tokens.remove(at);
        }
        (!after_skip(source, tokens, self, skipped)).then_some(d)
    }

    /// Ends every open interpolation at the line end or the end of the file at `at`, and returns
    /// `at`, or `None` when none is open. `end_of_file` when the end of the file stops them.
    fn end_line(
        &mut self,
        at: usize,
        end_of_file: bool,
        tokens: &mut Vec<Token>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<usize> {
        // A literal cut off before its first `{` is a frame whose brace is not open.
        let interpolation = self.frames.iter().any(|f| f.brace < self.open.len());
        let d = |quote| literal::not_closed(quote, interpolation, end_of_file);
        self.cut(at, d, tokens, diagnostics)
    }

    /// Ends every open interpolation at the comment at `at`, which takes the rest of the line, so
    /// no interpolation on it is closed. Returns `at`, or `None` when none is open.
    fn comment(
        &mut self,
        at: usize,
        tokens: &mut Vec<Token>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<usize> {
        let d = |_| {
            DiagnosticKind::CommentInInterpolation {
                at: Span::new(at, at + 2),
            }
            .into()
        };
        self.cut(at, d, tokens, diagnostics)
    }

    /// Ends every open interpolation at `at` with the error `d` gives for the innermost
    /// literal's `"`, and returns `at`, or `None` when none is open. The outermost literal's
    /// first token, and every token after it, become one `Str` token, so that no bracket opened
    /// in an interpolation stays open. When the cut ends that first token's own text, the token
    /// is not pushed yet, and `string` returns it.
    fn cut(
        &mut self,
        at: usize,
        d: impl FnOnce(usize) -> Diagnostic,
        tokens: &mut Vec<Token>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<usize> {
        let (outer, inner) = (self.frames.first()?, self.frames.last()?);
        unclosed(outer.before, d(inner.quote), diagnostics);
        self.trailing_semicolons = 0;
        self.open.truncate(outer.brace);
        tokens.truncate(outer.token + 1);
        if let Some(t) = tokens.get_mut(outer.token) {
            t.kind = TokenKind::Str(String::new());
        }
        self.frames.clear();
        Some(at)
    }

    /// The token of a string literal's text after its `"` at `start`, or after the `}` at
    /// `start` that ends an interpolation in it, and `start` when the text is not closed. A `{`
    /// that opens an interpolation pushes its frame and opens a bracket, and a literal that is
    /// not closed closes every frame, so that it is reported once. There is no token when that
    /// cut leaves an outer literal's token as the literal's one token. After a `:` at `start`,
    /// the token is a format spec's.
    fn string(
        &mut self,
        source: &str,
        start: usize,
        chars: &mut Chars,
        tokens: &mut Vec<Token>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> (Option<TokenKind>, Option<usize>) {
        if source[start..].starts_with(':') {
            return self.format_spec(source, start, chars, tokens, diagnostics);
        }
        let resumed = source[start..].starts_with('}');
        let (quote, token, before) = match self.frames.pop_if(|_| resumed) {
            // The `}` closes the interpolation's `{`.
            Some(f) => {
                self.open.pop();
                (f.quote, f.token, f.before)
            }
            None => (start, tokens.len(), diagnostics.len()),
        };
        let (text, end) = literal::string_text(source, chars, diagnostics);
        let kind = match (&end, resumed) {
            (End::Brace, false) => TokenKind::StrHead(text),
            (End::Brace, true) => TokenKind::StrMiddle(text),
            (_, false) => TokenKind::Str(text),
            (_, true) => TokenKind::StrTail(text),
        };
        let frame = Frame {
            brace: self.open.len(),
            quote,
            token,
            before,
        };
        match end {
            End::Brace => {
                // The text ends just after its `{`.
                let brace = chars.peek().map_or(source.len(), |&(i, _)| i) - 1;
                self.open.push(('{', Span::new(brace, brace + 1)));
                self.frames.push(frame);
            }
            // The literal is the innermost frame while its end is reported.
            End::Open => {
                self.frames.push(frame);
                let pushed = self.frames.first().is_some_and(|f| f.token < tokens.len());
                let cut = self.end_line(start, chars.peek().is_none(), tokens, diagnostics);
                return ((!pushed).then_some(kind), cut);
            }
            End::Quote => {}
        }
        (Some(kind), None)
    }

    /// The format spec after the `:` at `start`, which ends an interpolation's expression. A spec
    /// that a `"` or the line end cuts off leaves its literal unclosed and is not read: there is
    /// no token, and the rest of the line is lost in the literal.
    fn format_spec(
        &mut self,
        source: &str,
        start: usize,
        chars: &mut Chars,
        tokens: &mut Vec<Token>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> (Option<TokenKind>, Option<usize>) {
        let Some(end) = spec_end(source, start) else {
            while chars.next_if(|&(_, c)| c != '\n').is_some() {}
            return (
                None,
                self.end_line(start, chars.peek().is_none(), tokens, diagnostics),
            );
        };
        while chars.next_if(|&(i, _)| i < end).is_some() {}
        (
            Some(TokenKind::FormatSpec(source[start + 1..end].to_string())),
            None,
        )
    }
}

/// Reports `d`, an error that leaves a string literal without its end, unless an earlier error
/// in the literal is reported. The literal's errors are those after the first `before`
/// diagnostics. An error about a brace in its text yields to `d`, since the literal's end
/// decides what a brace means.
fn unclosed(before: usize, d: Diagnostic, diagnostics: &mut Vec<Diagnostic>) {
    let own = diagnostics.split_off(before);
    diagnostics.extend(
        own.into_iter()
            .filter(|e| !matches!(e.code(), Code::BraceInString | Code::EmptyInterpolation)),
    );
    if diagnostics.len() == before {
        diagnostics.push(d);
    }
}

/// The `Eof` after `tokens`: just after the last code, so "found end of file" points at its line.
fn eof(tokens: &[Token]) -> Token {
    let after_code = tokens
        .iter()
        .rfind(|t| t.kind != TokenKind::Newline)
        .map_or(0, |t| t.span.end());
    Token {
        kind: TokenKind::Eof,
        span: Span::new(after_code, after_code),
    }
}

/// The diagnostic a bracket error leaves, when no literal was left open before it: the first of
/// `diagnostics`, since a skip may have caused the bracket error, or else `bracket`.
fn first_or(diagnostics: Vec<Diagnostic>, bracket: Diagnostic) -> Diagnostic {
    diagnostics.into_iter().next().unwrap_or(bracket)
}

/// Whether text was `skipped` on the line of the last code token or after it. A line-start
/// diagnostic then reads text that recovery may have dropped, so it may be a consequence of the
/// skip's own diagnostic, and is not reported. In a block, a `;` is not code.
fn after_skip(source: &str, tokens: &[Token], brackets: &Brackets, skipped: Option<usize>) -> bool {
    let code = last_code(tokens, brackets.in_block());
    skipped.zip(code).is_some_and(|(s, code)| {
        // `None` when the skip comes after the code token.
        source
            .get(s..code.span.start())
            .is_none_or(|before| !before.contains('\n'))
    })
}

/// The offset in `rest` of the code after the line break at the start of `rest`, past
/// whitespace, blank lines and `//` comments.
fn next_code(rest: &str) -> usize {
    let mut code = rest;
    loop {
        // Only what the main loop skips: `trim_start` would also take Unicode whitespace.
        code = code.trim_start_matches([' ', '\t', '\r', '\n']);
        match code.strip_prefix("//") {
            Some(comment) => code = comment.split_once('\n').map_or("", |(_, after)| after),
            None => break,
        }
    }
    #[cfg(test)]
    LOOKED.set(LOOKED.get() + rest.len() - code.len());
    rest.len() - code.len()
}

/// The offset in `rest` of the code that continues the line break at the start of `rest`, which
/// `ends_statement` judged, past the gap `next_code` finds: a `.` or `?.`, or a name, `,` or `>`
/// that goes on with a header's type-parameter list (`continues_params`). `None` if code of
/// another kind follows.
fn continues_line(rest: &str, tokens: &[Token], brackets: &mut Brackets) -> Option<usize> {
    let gap = next_code(rest);
    let code = &rest[gap..];
    // `..` starts a range, not a continuation.
    let dot = code.starts_with('.') && !code.starts_with("..") || code.starts_with("?.");
    (dot || continues_params(code, tokens, brackets)).then_some(gap)
}

/// The last token that is not a `Newline`, nor, in a block, a `;` that separates statements: the
/// token the source would end with without them.
fn last_code(tokens: &[Token], in_block: bool) -> Option<&Token> {
    tokens
        .iter()
        .rfind(|t| !(t.kind == TokenKind::Newline || in_block && t.kind == TokenKind::Semicolon))
}

/// The token before the last `trailing` tokens, which are `;`s that separate statements: the one
/// a line break there would follow without them. Found by indexing, so that a long run of `;`
/// lines costs no more than a short one.
fn before_semicolons(tokens: &[Token], trailing: usize) -> Option<&Token> {
    tokens.get(tokens.len().checked_sub(trailing + 1)?)
}

fn newline(span: Span) -> Token {
    Token {
        kind: TokenKind::Newline,
        span,
    }
}

/// Whether a line break here ends a statement: the token before the last `trailing` tokens, which
/// are `;`s that separate statements, can end one, and the innermost open bracket, if any, is a
/// block. A `;` is transparent, so `x;\n` ends a statement as `x\n` does, and `{;\n` does not.
/// A `>` ends one too when it closes a type annotation (`annotates_type`).
fn ends_statement(tokens: &[Token], brackets: &mut Brackets) -> bool {
    let trailing = brackets.trailing_semicolons;
    let last = before_semicolons(tokens, trailing);
    brackets.open.last().is_none_or(|&(c, _)| c == '{')
        && (can_end_statement(last)
            || last.is_some_and(|t| matches!(t.kind, TokenKind::Gt))
                && brackets
                    .statements
                    .annotates_type(&tokens[..tokens.len() - trailing]))
}

/// Whether `code`, after a line break, goes on with the header's type-parameter list open before
/// it: `code` starts with a name, `,` or `>`. A `;` before the line break ends the list's
/// statement.
fn continues_params(code: &str, tokens: &[Token], brackets: &mut Brackets) -> bool {
    let word = &code[..code
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(code.len())];
    let listed = code.starts_with(',')
        || code.starts_with('>') && !code.starts_with(">=")
        || word.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && matches!(keyword_or_ident(word), TokenKind::Ident(_));
    listed
        && brackets.trailing_semicolons == 0
        && brackets.statements.read(tokens, tokens.len()).params
}

#[cfg(test)]
thread_local! {
    /// How many tokens `Statements::read` has read on this thread.
    static JUDGED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many bytes `next_code` has looked through on this thread.
    static LOOKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Whether `token` can end a statement (Go's rule).
fn can_end_statement(token: Option<&Token>) -> bool {
    use TokenKind::{
        Break, Char, Continue, False, Float, Ident, Int, NoneKw, Question, RBrace, RBracket,
        RParen, Return, SelfKw, Str, StrTail, Todo, True, Underscore,
    };
    token.is_some_and(|t| {
        matches!(
            t.kind,
            Ident(_)
                | Underscore
                | Int(_)
                | Float(_)
                | Char(_)
                | Str(_)
                | StrTail(_)
                | True
                | False
                | Return
                | Todo
                | Break
                | Continue
                | NoneKw
                | SelfKw
                | Question
                | RParen
                | RBracket
                | RBrace
        )
    })
}

/// Layout rules 2 and 3 where the parser cannot see them; `kind` starts a line.
/// A `{` only ever follows `fn …`, `if …`, `else`, `match …` or `while …` on its own line, so a
/// line-initial `{` after a token that could end a statement, after `else`, or after the `>`
/// of a `fn`, `class` or `enum` header's type arguments, breaks rule 2 at any depth; after a
/// `>` that compares, the `{` is its operand. Inside `( )` and `[ ]`, where no `Newline`
/// reaches the parser, so does a line-initial `else` (rule 3), and a line-initial `[` after a
/// token that could end a statement, which would silently index the line above.
fn check_line_start(
    source: &str,
    tokens: &[Token],
    open: &[Open],
    statements: &mut Statements,
    kind: &TokenKind,
    span: Span,
) -> Option<Diagnostic> {
    match kind {
        TokenKind::LBrace => {
            let in_block = open.last().is_none_or(|&(c, _)| c == '{');
            let code = last_code(tokens, in_block);
            match code {
                Some(code)
                    if can_end_statement(Some(code))
                        || matches!(code.kind, TokenKind::Else | TokenKind::Loop)
                        || code.kind == TokenKind::Gt && statements.closes_header(tokens) =>
                {
                    // Only a header takes the `{`: joining it to another statement almost never
                    // gives a valid one (`print(1) {`, `let x = 1 {`, `} {`). Inside `(` or `[`
                    // the moved `{` would still sit in the unclosed bracket.
                    let statement = match last_statement(tokens) {
                        // A loop label: the header starts after it.
                        [label, colon, header @ ..]
                            if matches!(label.kind, TokenKind::Ident(_))
                                && colon.kind == TokenKind::Colon =>
                        {
                            header
                        }
                        statement => statement,
                    };
                    let join = (in_block
                        && statement.first().is_some_and(is_header)
                        && header_awaits_brace(statement))
                    .then(|| Join {
                        gap: Span::new(code.span.end(), span.start()),
                        // The join deletes the gap, so a comment in it makes the join unsafe.
                        safe: header_awaits_brace(line_of(source, tokens, code))
                            && source[code.span.end()..span.start()].trim().is_empty(),
                    });
                    Some(DiagnosticKind::BraceOnNewLine { at: span, join }.into())
                }
                _ => None,
            }
        }
        TokenKind::LBracket
            if open.last().is_some_and(|&(c, _)| c != '{') && can_end_statement(tokens.last()) =>
        {
            Some(DiagnosticKind::BracketOnNewLine { at: span }.into())
        }
        TokenKind::Else
            if open.last().is_some_and(|&(c, _)| c != '{') && can_end_statement(tokens.last()) =>
        {
            // Only a `}` can take the `else`; the lexer cannot tell whether its `if` has one.
            let join = tokens
                .last()
                .filter(|t| t.kind == TokenKind::RBrace)
                .map(|brace| Join {
                    gap: Span::new(brace.span.end(), span.start()),
                    safe: false,
                });
            Some(DiagnosticKind::ElseOnNewLine { at: span, join }.into())
        }
        _ => None,
    }
}

/// The tokens on the line of `code`, up to the end of `tokens`.
fn line_of<'t>(source: &str, tokens: &'t [Token], code: &Token) -> &'t [Token] {
    let before = tokens.iter().rposition(|t| {
        source[t.span.start().min(code.span.start())..code.span.start()].contains('\n')
    });
    &tokens[before.map_or(0, |i| i + 1)..]
}

/// The statement that `tokens` ends with in a block, before any line break or `;` after it: back
/// to the last line break or `;` that ended a statement, or to the `{` of the block it stands in.
/// A break inside brackets or before a continuation never reached `tokens`, so a statement can
/// span lines, and a `;` inside `( )`, `[ ]` or an interpolation does not end it.
fn last_statement(tokens: &[Token]) -> &[Token] {
    let end = tokens
        .iter()
        .rposition(|t| !matches!(t.kind, TokenKind::Newline | TokenKind::Semicolon))
        .map_or(0, |i| i + 1);
    let mut depth = 0_usize;
    let start = tokens[..end].iter().rposition(|t| match t.kind {
        TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket | TokenKind::StrTail(_) => {
            depth += 1;
            false
        }
        TokenKind::LBrace if depth > 0 => {
            depth -= 1;
            false
        }
        TokenKind::LParen | TokenKind::LBracket | TokenKind::StrHead(_) => {
            depth = depth.saturating_sub(1);
            false
        }
        TokenKind::LBrace | TokenKind::Newline | TokenKind::Semicolon => depth == 0,
        _ => false,
    });
    &tokens[start.map_or(0, |i| i + 1)..end]
}

/// Whether `t` starts a header, which takes a `{` at the end of its line.
fn is_header(t: &Token) -> bool {
    matches!(
        t.kind,
        TokenKind::Fn
            | TokenKind::Class
            | TokenKind::Enum
            | TokenKind::If
            | TokenKind::Match
            | TokenKind::Else
            | TokenKind::While
            | TokenKind::For
            | TokenKind::Loop
    )
}

/// Whether `line`, the code before a `{` that starts a line, holds a header keyword that has no
/// `{` or `}` after it yet: the `{` then opens that header. A keyword before a `}` either has its
/// block already or sits in a closed one.
fn header_awaits_brace(line: &[Token]) -> bool {
    line.iter()
        .rev()
        .find_map(|t| match t.kind {
            TokenKind::LBrace | TokenKind::RBrace => Some(false),
            _ => is_header(t).then_some(true),
        })
        .unwrap_or(false)
}

/// The innermost bracket still open at the end of the file, unless a literal was not closed: it
/// may be one whose closer the literal ate, and its error is already reported.
fn unclosed_at_end(open: &[Open], diagnostics: &[Diagnostic]) -> Option<Open> {
    open.last()
        .copied()
        .filter(|_| !after_unclosed_literal(diagnostics))
}

/// Whether a literal was not closed, as `diagnostics` say.
fn after_unclosed_literal(diagnostics: &[Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|d| d.code() == Code::LiteralNotClosed)
}

/// Tracks the bracket `kind` at `span` (`track_bracket`), and returns whether its token stays.
/// After a literal that is not closed, a closer pops down to its opener, and the brackets it pops
/// get a closer of no width before it, so that the tokens stay balanced. No closer pops past the
/// innermost interpolation's `{`.
fn track_closer(
    brackets: &mut Brackets,
    kind: &TokenKind,
    span: Span,
    diagnostics: &[Diagnostic],
    tokens: &mut Vec<Token>,
) -> Result<bool, Diagnostic> {
    let floor = brackets.frames.last().map_or(0, |f| f.brace + 1);
    let (literal, read) = (&mut brackets.literal, &mut brackets.read);
    // Read only for a closer that does not match, and only the diagnostics added since, so that
    // a source with many errors is not scanned once per token. `unclosed` drops diagnostics of
    // the literal it lexes only, before any closer after it, so the read ones stay.
    let recover = || {
        let new = diagnostics.get(*read..).unwrap_or_default();
        if let Some(d) = new.iter().rfind(|d| d.code() == Code::LiteralNotClosed) {
            *literal = Some(d.span().start());
        }
        *read = diagnostics.len();
        literal.map(|at| (floor, at))
    };
    let Some(left) = track_bracket(&mut brackets.open, kind, span, recover)? else {
        return Ok(false);
    };
    for (bracket, _) in left.into_iter().rev() {
        let kind = match bracket {
            '(' => TokenKind::RParen,
            '[' => TokenKind::RBracket,
            _ => TokenKind::RBrace,
        };
        let at = Span::new(span.start(), span.start());
        tokens.push(Token { kind, span: at });
    }
    Ok(true)
}

/// Pushes an opening bracket onto `open`, or checks that a closing one matches the top. Returns
/// `None` when the token is dropped, else the brackets to close before it. When a closer does not
/// match and `recover` gives the lowest index of `open` that a closer may pop to and where the
/// latest literal that is not closed starts, the closer pops down to the nearest opener it
/// matches, and returns the openers above it, since the literal ate their closers. It ate none of
/// a bracket opened after it, so the closer of one is still an error. A closer with no such
/// opener is a stray: it is dropped, and the error was already reported.
fn track_bracket(
    open: &mut Vec<Open>,
    kind: &TokenKind,
    span: Span,
    recover: impl FnOnce() -> Option<(usize, usize)>,
) -> Result<Option<Vec<Open>>, Diagnostic> {
    let (closes, c) = match kind {
        TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
            let c = match kind {
                TokenKind::LParen => '(',
                TokenKind::LBracket => '[',
                _ => '{',
            };
            open.push((c, span));
            return Ok(Some(Vec::new()));
        }
        TokenKind::RParen => ('(', ')'),
        TokenKind::RBracket => ('[', ']'),
        TokenKind::RBrace => ('{', '}'),
        _ => return Ok(Some(Vec::new())),
    };
    if let Some((o, _)) = open.last()
        && *o == closes
    {
        open.pop();
        return Ok(Some(Vec::new()));
    }
    if let Some((floor, literal)) = recover() {
        let found = open
            .iter()
            .rposition(|&(o, _)| o == closes)
            .filter(|&i| i >= floor);
        let eaten = open.last().is_some_and(|&(_, at)| at.start() < literal);
        if found.is_none() || eaten {
            return Ok(found.map(|i| open.split_off(i).split_off(1)));
        }
    }
    Err(DiagnosticKind::UnexpectedClosingBracket {
        bracket: c,
        at: span,
        opened: open.pop(),
    }
    .into())
}

/// One token per line, `line:col Kind text`; `Newline` and `Eof` print without text.
pub(crate) fn dump(source: &str, tokens: &[Token]) -> String {
    let mut out = String::new();
    let lines = LineIndex::new(source);
    for t in tokens {
        let (line, col) = lines.line_col(source, t.span.start());
        write!(out, "{line}:{col} {}", t.kind.name())
            .expect("writing to a `String` should never fail");
        if !matches!(t.kind, TokenKind::Newline | TokenKind::Eof) {
            write!(out, " {}", &source[t.span.range()])
                .expect("writing to a `String` should never fail");
        }
        out.push('\n');
    }
    out
}

/// Where the format spec whose `:` is at `colon` ends: at the `}` that ends the interpolation, or
/// `None` when a `"` or the line end cuts it off. A `{` in it opens a pair whose `}` is part of
/// the spec.
fn spec_end(source: &str, colon: usize) -> Option<usize> {
    let mut open = 0_usize;
    // Each byte searched for is ASCII, so it is never part of a longer char.
    for (i, b) in source.bytes().enumerate().skip(colon) {
        match b {
            b'"' | b'\n' => return None,
            b'{' => open += 1,
            b'}' if open == 0 => return Some(i),
            b'}' => open -= 1,
            _ => {}
        }
    }
    None
}

/// Consumes chars while `pred` holds and returns the byte offset after the last one.
fn take_while(chars: &mut Chars, source: &str, pred: fn(char) -> bool) -> usize {
    while chars.next_if(|&(_, c)| pred(c)).is_some() {}
    chars.peek().map_or(source.len(), |&(i, _)| i)
}

fn keyword_or_ident(text: &str) -> TokenKind {
    match text {
        "fn" => TokenKind::Fn,
        "let" => TokenKind::Let,
        "mut" => TokenKind::Mut,
        "var" => TokenKind::Var,
        "import" => TokenKind::Reserved("import"),
        "pub" => TokenKind::Reserved("pub"),
        "record" => TokenKind::Reserved("record"),
        "type" => TokenKind::Reserved("type"),
        "weak" => TokenKind::Reserved("weak"),
        "test" => TokenKind::Reserved("test"),
        "todo" => TokenKind::Todo,
        "echo" => TokenKind::Reserved("echo"),
        "interface" => TokenKind::Reserved("interface"),
        "extern" => TokenKind::Reserved("extern"),
        "as" => TokenKind::Reserved("as"),
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "while" => TokenKind::While,
        "for" => TokenKind::For,
        "loop" => TokenKind::Loop,
        "in" => TokenKind::In,
        "return" => TokenKind::Return,
        "break" => TokenKind::Break,
        "continue" => TokenKind::Continue,
        "class" => TokenKind::Class,
        "const" => TokenKind::Const,
        "enum" => TokenKind::Enum,
        "match" => TokenKind::Match,
        "None" => TokenKind::NoneKw,
        "is" => TokenKind::Is,
        "self" => TokenKind::SelfKw,
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        "_" => TokenKind::Underscore,
        _ => TokenKind::Ident(Symbol::intern(text)),
    }
}

/// The magnitude of `i64::MIN`, the one literal that fits `i64` only after a unary `-`.
pub(crate) const MIN_MAGNITUDE: u64 = 1 << 63;

/// The one error for a literal that no `i64` holds, wherever it is found.
pub(crate) fn literal_too_large(at: Span) -> Diagnostic {
    DiagnosticKind::IntegerTooLarge { at }.into()
}

/// The operator or punctuation starting with `c`, consuming a second char when it has one.
fn symbol(c: char, chars: &mut Chars) -> Option<TokenKind> {
    use TokenKind::{
        AndAnd, Arrow, Bang, Colon, Comma, Dot, DotDot, DotDotEq, Eq, EqEq, FatArrow, Ge, Gt,
        LBrace, LBracket, LParen, Le, Lt, Minus, MinusEq, NotEq, OrOr, Percent, PercentEq, Pipe,
        Plus, PlusEq, QuestionDot, QuestionQuestion, RBrace, RBracket, RParen, Semicolon, Slash,
        SlashEq, Star, StarEq, StarStar,
    };
    let mut eat = |want| chars.next_if(|&(_, n)| n == want).is_some();
    Some(match c {
        '+' if eat('=') => PlusEq,
        '+' => Plus,
        '-' if eat('=') => MinusEq,
        '-' if eat('>') => Arrow,
        '-' => Minus,
        '*' if eat('=') => StarEq,
        '*' if eat('*') => StarStar,
        '*' => Star,
        '/' if eat('=') => SlashEq,
        '/' => Slash,
        '%' if eat('=') => PercentEq,
        '%' => Percent,
        '=' if eat('=') => EqEq,
        '=' if eat('>') => FatArrow,
        '=' => Eq,
        '!' if eat('=') => NotEq,
        '!' => Bang,
        '<' if eat('=') => Le,
        '<' => Lt,
        '>' if eat('=') => Ge,
        '>' => Gt,
        '&' if eat('&') => AndAnd,
        '|' if eat('|') => OrOr,
        '|' => Pipe,
        '(' => LParen,
        ')' => RParen,
        '{' => LBrace,
        '}' => RBrace,
        ',' => Comma,
        ':' => Colon,
        '.' if eat('.') => {
            if eat('=') {
                DotDotEq
            } else {
                DotDot
            }
        }
        '.' => Dot,
        '[' => LBracket,
        ']' => RBracket,
        ';' => Semicolon,
        '?' if eat('.') => QuestionDot,
        '?' if eat('?') => QuestionQuestion,
        '?' => TokenKind::Question,
        _ => return None,
    })
}

fn unexpected(c: char, at: usize) -> Diagnostic {
    // The span covers the whole char: annotate-snippets panics off a char boundary.
    DiagnosticKind::UnexpectedCharacter {
        c,
        at: Span::new(at, at + c.len_utf8()),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Code;
    use crate::diagnostic::explanations::{apply_fix, fix_edits};
    use crate::source::{LineIndex, Span, line_col};

    /// The tokens of `src`, which lexes without a diagnostic.
    fn tokens(src: &str) -> Vec<Token> {
        lex_clean(src).expect("source lexes")
    }

    /// The diagnostics of `src`, which the lexer recovers from.
    fn recovered(src: &str) -> Vec<Diagnostic> {
        let lexed = lex(src).expect("the lexer recovers");
        assert!(!lexed.diagnostics.is_empty(), "{src} lexes clean");
        lexed.diagnostics
    }

    /// The one diagnostic of `src`, which stops the lexer.
    fn fatal(src: &str) -> Diagnostic {
        lex(src).expect_err("the lexer stops")
    }

    /// The one diagnostic of `src`, which the lexer recovers from.
    fn only(src: &str) -> Diagnostic {
        let ds = recovered(src);
        let [d] = <[Diagnostic; 1]>::try_from(ds).expect("one diagnostic");
        d
    }

    fn at(src: &str, d: &Diagnostic) -> (Code, usize, usize) {
        let (line, col) = line_col(src, d.span().start());
        (d.code(), line, col)
    }

    fn err(src: &str) -> (Code, usize, usize) {
        at(src, &only(src))
    }

    fn kinds(src: &str) -> Vec<TokenKind> {
        tokens(src)
            .into_iter()
            .map(|t| t.kind)
            .filter(|k| *k != TokenKind::Newline)
            .collect()
    }

    fn ident(s: &str) -> TokenKind {
        TokenKind::Ident(Symbol::intern(s))
    }

    fn int(value: u64) -> TokenKind {
        TokenKind::Int(value)
    }

    #[test]
    fn keywords_and_symbols() {
        use TokenKind::{Arrow, Colon, Eof, Fn, LBrace, LParen, RBrace, RParen};
        assert_eq!(
            kinds("fn f(a: i64) -> bool { a }"),
            [
                Fn,
                ident("f"),
                LParen,
                ident("a"),
                Colon,
                ident("i64"),
                RParen,
                Arrow,
                ident("bool"),
                LBrace,
                ident("a"),
                RBrace,
                Eof
            ]
        );
    }

    #[test]
    fn all_keywords() {
        use TokenKind::{Const, Else, Eof, False, Fn, If, Let, Mut, Return, True, Var, While};
        assert_eq!(
            kinds("fn let mut var const if else while return true false"),
            [
                Fn, Let, Mut, Var, Const, If, Else, While, Return, True, False, Eof
            ]
        );
    }

    #[test]
    fn reserved_words() {
        use TokenKind::{Eof, Reserved};
        let words = [
            "import",
            "pub",
            "record",
            "type",
            "weak",
            "test",
            "echo",
            "interface",
            "extern",
            "as",
        ];
        let mut expected: Vec<_> = words.iter().map(|w| Reserved(w)).collect();
        expected.extend([
            ident("matches"),
            ident("type_"),
            ident("pubx"),
            ident("ask"),
        ]);
        expected.push(Eof);
        assert_eq!(
            kinds(&format!("{} matches type_ pubx ask", words.join(" "))),
            expected
        );
        // `enum` and `match` are keywords now.
        assert_eq!(
            kinds("enum enums match matches"),
            [
                TokenKind::Enum,
                ident("enums"),
                TokenKind::Match,
                ident("matches"),
                Eof
            ]
        );
    }

    #[test]
    fn pipe_and_range_tokens() {
        use TokenKind::{DotDot, DotDotEq, Eof, OrOr, Pipe};
        assert_eq!(
            kinds("a | b || c"),
            [ident("a"), Pipe, ident("b"), OrOr, ident("c"), Eof]
        );
        assert_eq!(kinds("a..=b"), [ident("a"), DotDotEq, ident("b"), Eof]);
        assert_eq!(kinds("a..b"), [ident("a"), DotDot, ident("b"), Eof]);
        assert_eq!(kinds("a ..= b"), [ident("a"), DotDotEq, ident("b"), Eof]);
    }

    /// A `|` cannot end a statement, so a line that ends with one continues onto the next.
    #[test]
    fn a_line_ending_in_a_pipe_continues() {
        use TokenKind::{Eof, FatArrow, Newline, Pipe};
        assert_eq!(
            tokens("A |\nB =>\n")
                .into_iter()
                .map(|t| t.kind)
                .collect::<Vec<_>>(),
            [ident("A"), Pipe, ident("B"), FatArrow, Eof]
        );
        // A `|` that starts the line follows a statement that ends.
        assert_eq!(
            tokens("A\n| B")
                .into_iter()
                .map(|t| t.kind)
                .collect::<Vec<_>>(),
            [ident("A"), Newline, Pipe, ident("B"), Newline, Eof]
        );
    }

    #[test]
    fn fat_arrow_is_one_token() {
        use TokenKind::{Eof, Eq, FatArrow, Gt, Newline};
        assert_eq!(kinds("a => b"), [ident("a"), FatArrow, ident("b"), Eof]);
        assert_eq!(kinds("a=>b"), [ident("a"), FatArrow, ident("b"), Eof]);
        assert_eq!(
            kinds("a = > b"),
            [ident("a"), Eq, Gt, ident("b"), Eof],
            "a gap splits the token"
        );
        // `=>` joins the next line, as a binary operator does, so no `Newline` follows it.
        assert_eq!(
            all_kinds("a =>\nb"),
            [ident("a"), FatArrow, ident("b"), Newline, Eof]
        );
    }

    #[test]
    fn todo_token() {
        use TokenKind::{Eof, Newline, Todo};
        assert_eq!(all_kinds("todo\ntodo"), [Todo, Newline, Todo, Newline, Eof]);
    }

    #[test]
    fn underscore_token() {
        use TokenKind::{Eof, Underscore};
        assert_eq!(
            kinds("_ _x x_"),
            [Underscore, ident("_x"), ident("x_"), Eof]
        );
    }

    #[test]
    fn break_and_continue_end_a_statement() {
        use TokenKind::{Break, Continue, Eof, Newline};
        assert_eq!(
            all_kinds("break\ncontinue"),
            [Break, Newline, Continue, Newline, Eof]
        );
    }

    #[test]
    fn class_and_optional_tokens() {
        use TokenKind::{
            Class, Colon, Dot, Eof, Is, Let, Mut, NoneKw, Question, QuestionDot, QuestionQuestion,
            SelfKw,
        };
        assert_eq!(
            kinds("class None is self"),
            [Class, NoneKw, Is, SelfKw, Eof]
        );
        assert_eq!(kinds("x?.f"), [ident("x"), QuestionDot, ident("f"), Eof]);
        assert_eq!(
            kinds("x ?? y"),
            [ident("x"), QuestionQuestion, ident("y"), Eof]
        );
        assert_eq!(kinds("Tree??"), [ident("Tree"), QuestionQuestion, Eof]);
        assert_eq!(kinds("Tree? ?"), [ident("Tree"), Question, Question, Eof]);
        assert_eq!(kinds("a.b"), [ident("a"), Dot, ident("b"), Eof]);
        assert_eq!(
            all_kinds("mut next: Tree?\nlet"),
            [
                Mut,
                ident("next"),
                Colon,
                ident("Tree"),
                Question,
                TokenKind::Newline,
                Let,
                Eof
            ]
        );
    }

    #[test]
    fn none_and_self_end_a_statement() {
        use TokenKind::{Eof, Eq, Newline, NoneKw, SelfKw};
        assert_eq!(
            all_kinds("x = None\ny"),
            [ident("x"), Eq, NoneKw, Newline, ident("y"), Newline, Eof]
        );
        assert_eq!(
            all_kinds("self\ny"),
            [SelfKw, Newline, ident("y"), Newline, Eof]
        );
    }

    #[test]
    fn dot_and_question_dot_continue_a_line() {
        use TokenKind::{
            Dot, Else, Eof, Eq, If, LBrace, LParen, Let, Newline, QuestionDot, QuestionQuestion,
            RBrace, RParen,
        };
        assert_eq!(
            all_kinds("let t = a\n    .sum()"),
            [
                Let,
                ident("t"),
                Eq,
                ident("a"),
                Dot,
                ident("sum"),
                LParen,
                RParen,
                Newline,
                Eof
            ]
        );
        assert_eq!(
            all_kinds("a\r\n.b"),
            [ident("a"), Dot, ident("b"), Newline, Eof]
        );
        assert_eq!(
            all_kinds("a\r\n\r\n.b"),
            [ident("a"), Dot, ident("b"), Newline, Eof]
        );
        assert_eq!(all_kinds("a\n// c"), [ident("a"), Newline, Eof]);
        assert_eq!(
            all_kinds("a\n\n    // c\n    ?.b"),
            [ident("a"), QuestionDot, ident("b"), Newline, Eof]
        );
        assert_eq!(
            all_kinds("a\n?? b"),
            [
                ident("a"),
                Newline,
                QuestionQuestion,
                ident("b"),
                Newline,
                Eof
            ]
        );
        assert_eq!(all_kinds("a\n."), [ident("a"), Dot, Eof]);
        // Inside brackets: a block in parentheses, and parentheses alone.
        assert_eq!(
            all_kinds("f(if c {\n a\n .b\n } else { 0 })"),
            [
                ident("f"),
                LParen,
                If,
                ident("c"),
                LBrace,
                ident("a"),
                Dot,
                ident("b"),
                Newline,
                RBrace,
                Else,
                LBrace,
                int(0),
                RBrace,
                RParen,
                Newline,
                Eof
            ]
        );
        assert_eq!(
            all_kinds("(a\n.b)"),
            [LParen, ident("a"), Dot, ident("b"), RParen, Newline, Eof]
        );
    }

    #[test]
    fn all_operators() {
        use TokenKind::{
            AndAnd, Bang, Comma, Eof, Eq, EqEq, Ge, Gt, Le, Lt, Minus, MinusEq, NotEq, OrOr,
            Percent, PercentEq, Plus, PlusEq, Slash, SlashEq, Star, StarEq,
        };
        assert_eq!(
            kinds("+ - * / % += -= *= /= %= == != < <= > >= && || ! = ,"),
            [
                Plus, Minus, Star, Slash, Percent, PlusEq, MinusEq, StarEq, SlashEq, PercentEq,
                EqEq, NotEq, Lt, Le, Gt, Ge, AndAnd, OrOr, Bang, Eq, Comma, Eof
            ]
        );
    }

    #[test]
    fn star_star() {
        use TokenKind::{Eof, Eq, Star, StarEq, StarStar};
        assert_eq!(kinds("** * *="), [StarStar, Star, StarEq, Eof]);
        // `**=` is `**` and then `=`, which the parser skips.
        assert_eq!(kinds("**="), [StarStar, Eq, Eof]);
        assert_eq!(kinds("***"), [StarStar, Star, Eof]);
    }

    #[test]
    fn digit_separators() {
        assert_eq!(kinds("100_000"), [int(100_000), TokenKind::Eof]);
    }

    fn float(value: f64) -> TokenKind {
        TokenKind::Float(Float(value))
    }

    /// Each literal is the nearest `f64` to its text; the expected values are Rust literals.
    #[test]
    fn float_literals() {
        for (src, expected) in [
            ("1.0", 1.0),
            ("0.5", 0.5),
            ("9.81", 9.81),
            ("6.674e-11", 6.674e-11),
            ("1.5E+3", 1500.0),
            ("1e9", 1e9),
            ("2e-3", 0.002),
            ("1_000.25", 1000.25),
            ("1e1_0", 1e10),
            ("0.1", 0.1),
            ("1e-400", 0.0),
        ] {
            assert_eq!(kinds(src), [float(expected), TokenKind::Eof], "{src}");
        }
    }

    /// A float literal is one token that spans all its text.
    #[test]
    fn float_literal_span() {
        let t = &tokens("1_0.5e+1_0")[0];
        assert_eq!((t.span.start(), t.span.end()), (0, 10));
    }

    /// A `.` belongs to a number only when a digit follows it, and a digit run right after a
    /// `.` or `?.` is an integer.
    #[test]
    fn dot_after_integer() {
        use TokenKind::{Dot, DotDot, Eof, LParen, QuestionDot, RParen};
        for (src, expected) in [
            ("0..n", vec![int(0), DotDot, ident("n"), Eof]),
            (
                "1.max(2)",
                vec![int(1), Dot, ident("max"), LParen, int(2), RParen, Eof],
            ),
            ("1.e5", vec![int(1), Dot, ident("e5"), Eof]),
            ("1._5", vec![int(1), Dot, ident("_5"), Eof]),
            ("t.0.1", vec![ident("t"), Dot, int(0), Dot, int(1), Eof]),
            ("a?.0", vec![ident("a"), QuestionDot, int(0), Eof]),
            (
                "a?.0.1",
                vec![ident("a"), QuestionDot, int(0), Dot, int(1), Eof],
            ),
            ("t.0e3", vec![ident("t"), Dot, int(0), ident("e3"), Eof]),
            ("1.5.max", vec![float(1.5), Dot, ident("max"), Eof]),
            ("1e5.max", vec![float(1e5), Dot, ident("max"), Eof]),
        ] {
            assert_eq!(kinds(src), expected, "{src}");
        }
    }

    /// Each error is at its text, which the last column measures; the lexer goes on after it.
    #[test]
    fn float_errors() {
        use Code::{DigitSeparator, ExponentNeedsDigits, FloatNeedsDigits, FloatOutOfRange};
        for (src, code, col, len) in [
            ("1.", FloatNeedsDigits, 2, 1),
            ("1. + 2", FloatNeedsDigits, 2, 1),
            ("f(1.)", FloatNeedsDigits, 4, 1),
            ("1.\n", FloatNeedsDigits, 2, 1),
            ("1_0.", FloatNeedsDigits, 4, 1),
            ("1e", ExponentNeedsDigits, 2, 1),
            ("1e+", ExponentNeedsDigits, 2, 2),
            ("1e-", ExponentNeedsDigits, 2, 2),
            ("1.5E", ExponentNeedsDigits, 4, 1),
            ("1.5e+ 2", ExponentNeedsDigits, 4, 2),
            ("1e400", FloatOutOfRange, 1, 5),
            ("1.5e+400", FloatOutOfRange, 1, 8),
            ("1_.5", DigitSeparator, 2, 1),
            ("1.5_", DigitSeparator, 4, 1),
            ("1e_5", DigitSeparator, 3, 1),
            ("1__0.5", DigitSeparator, 2, 1),
            ("1.5__0", DigitSeparator, 4, 1),
            ("1e5_", DigitSeparator, 4, 1),
            ("1e+_5", DigitSeparator, 4, 1),
        ] {
            let d = only(src);
            let (line, found_col) = line_col(src, d.span().start());
            assert_eq!(
                (d.code(), line, found_col, d.span().end() - d.span().start()),
                (code, 1, col, len),
                "{src}"
            );
        }
    }

    /// After an error the rest of the line lexes on, with a placeholder for the literal.
    #[test]
    fn a_float_error_leaves_a_placeholder() {
        use TokenKind::{Eof, Plus};
        for src in ["1. + 2", "1e + 2", "1e400 + 2", "1_.5 + 2"] {
            let lexed = lex(src).expect("the lexer recovers");
            let found: Vec<_> = lexed
                .tokens
                .into_iter()
                .map(|t| t.kind)
                .filter(|k| *k != TokenKind::Newline)
                .collect();
            assert_eq!(found, [int(0), Plus, int(2), Eof], "{src}");
        }
    }

    /// Only the dot's error is safe: it writes the `0` that is missing.
    #[test]
    fn float_dot_fix_is_safe() {
        for (src, fixed) in [("1.", "1.0"), ("1. + 2", "1.0 + 2"), ("12_3.", "12_3.0")] {
            let d = only(src);
            assert!(fix_edits(&d).is_some_and(|(safe, _)| safe), "{src}");
            assert_eq!(apply_fix(src, &d), fixed);
        }
        for src in ["1e", "1e400"] {
            assert!(fix_edits(&only(src)).is_none(), "{src}");
        }
    }

    /// A prefixed integer reads every letter as a digit, so there is no exponent in it.
    #[test]
    fn based_integers_have_no_exponent() {
        assert_eq!(kinds("0x1e5"), [TokenKind::Int(0x1e5), TokenKind::Eof]);
    }

    #[test]
    fn double_underscore() {
        assert_eq!(err("1__0"), (Code::DigitSeparator, 1, 2));
    }

    #[test]
    fn trailing_underscore() {
        assert_eq!(err("100_"), (Code::DigitSeparator, 1, 4));
    }

    #[test]
    fn max_literal() {
        assert_eq!(
            kinds("9223372036854775807"),
            [int(9_223_372_036_854_775_807), TokenKind::Eof]
        );
    }

    /// The magnitude of `i64::MIN` lexes: the parser decides whether a `-` precedes it.
    #[test]
    fn min_magnitude_literal() {
        assert_eq!(
            kinds("9223372036854775808"),
            [int(MIN_MAGNITUDE), TokenKind::Eof]
        );
    }

    #[test]
    fn bases() {
        let source = "0xFF 0xff 0b1010_0101 0o17 0x7FFF_FFFF_FFFF_FFFF 0";
        assert_eq!(
            kinds(source),
            [
                int(255),
                int(255),
                int(165),
                int(15),
                int(i64::MAX.cast_unsigned()),
                int(0),
                TokenKind::Eof
            ]
        );
    }

    /// As in decimal, the magnitude of `i64::MIN` lexes in any base: the parser decides.
    #[test]
    fn min_magnitude_in_other_bases() {
        assert_eq!(
            kinds("0x8000_0000_0000_0000"),
            [int(MIN_MAGNITUDE), TokenKind::Eof]
        );
    }

    /// An integer literal is one token that spans exactly its text as written: its prefix, its
    /// digits and its separators. The parser and checker quote `source[span]`.
    #[test]
    fn base_literal_span() {
        for (src, want) in [
            ("0xFF", (0, 4)),
            ("100_000", (0, 7)),
            ("0b1010_0101", (0, 11)),
            ("0x8000_0000_0000_0000", (0, 21)),
            ("0xff", (0, 4)),
            ("0o17", (0, 4)),
            ("0x7FFF_FFFF_FFFF_FFFF", (0, 21)),
            ("x 1_0", (2, 5)),
        ] {
            let t = tokens(src)
                .into_iter()
                .find(|t| matches!(t.kind, TokenKind::Int(_)))
                .expect("the source has an integer literal");
            assert_eq!((t.span.start(), t.span.end()), want, "{src}");
        }
    }

    #[test]
    fn base_errors() {
        use Code::{BasePrefixUpperCase, BasePrefixWithoutDigits, DigitOutsideBase};
        for (src, expected) in [
            ("0X1F", (BasePrefixUpperCase, 1, 1)),
            ("0B1", (BasePrefixUpperCase, 1, 1)),
            ("0O7", (BasePrefixUpperCase, 1, 1)),
            ("0x", (BasePrefixWithoutDigits, 1, 1)),
            ("0b", (BasePrefixWithoutDigits, 1, 1)),
            ("0o", (BasePrefixWithoutDigits, 1, 1)),
            ("0b102", (DigitOutsideBase, 1, 5)),
            ("0o9", (DigitOutsideBase, 1, 3)),
            ("0xFG", (DigitOutsideBase, 1, 4)),
            ("0x_FF", (Code::DigitSeparator, 1, 3)),
            ("0xFF_", (Code::DigitSeparator, 1, 5)),
            ("0x1_0000_0000_0000_0000", (Code::IntegerTooLarge, 1, 1)),
            // 2^64 in octal; the binary one follows the loop.
            (
                "0o2_000_000_000_000_000_000_000",
                (Code::IntegerTooLarge, 1, 1),
            ),
        ] {
            assert_eq!(err(src), expected, "{src}");
        }
        let binary = format!("0b1{}", "0".repeat(64));
        assert_eq!(err(&binary), (Code::IntegerTooLarge, 1, 1));
    }

    /// The upper case prefix is reported only when its fix would leave a clean literal.
    #[test]
    fn upper_case_prefix_yields_to_other_errors() {
        assert_eq!(err("0XG"), (Code::DigitOutsideBase, 1, 3));
        assert_eq!(err("0X"), (Code::BasePrefixWithoutDigits, 1, 1));
        assert_eq!(err("0B"), (Code::BasePrefixWithoutDigits, 1, 1));
        assert_eq!(err("0O"), (Code::BasePrefixWithoutDigits, 1, 1));
        assert_eq!(err("0X_1"), (Code::DigitSeparator, 1, 3));
        assert_eq!(
            err("0XFFFF_FFFF_FFFF_FFFF_F"),
            (Code::IntegerTooLarge, 1, 1)
        );
    }

    #[test]
    fn upper_case_prefix_fix_is_safe() {
        for (src, fixed) in [("0X1F", "0x1F"), ("0B1", "0b1"), ("0O7", "0o7")] {
            let d = only(src);
            assert!(fix_edits(&d).is_some_and(|(safe, _)| safe), "{src}");
            assert_eq!(apply_fix(src, &d), fixed);
        }
    }

    /// A bare 2^63 does not parse, so lower-casing its prefix is unsafe; after a `-` it is right.
    #[test]
    fn upper_case_prefix_fix_at_min_magnitude_is_unsafe() {
        use crate::front_end::front_end;
        let literal = "0X8000_0000_0000_0000";
        let d = only(literal);
        assert!(fix_edits(&d).is_some_and(|(safe, _)| !safe));
        for (sign, parses) in [("", false), ("-", true)] {
            let src = format!("fn main() {{ let _ = {sign}{literal} }}");
            let fixed = apply_fix(&src, &only(&src));
            let clean = front_end(&fixed).1.is_empty();
            assert_eq!(clean, parses, "{fixed}");
        }
        // One below it is safe: the fixed literal parses anywhere.
        let d = only("0X7FFF_FFFF_FFFF_FFFF");
        assert!(fix_edits(&d).is_some_and(|(safe, _)| safe));
    }

    /// A prefix letter after the `0` with a gap is an identifier, not a base.
    #[test]
    fn base_prefix_needs_no_gap() {
        assert_eq!(kinds("0 x"), [int(0), ident("x"), TokenKind::Eof]);
    }

    /// Digits right after a `.` or `?.` are a float's fraction or a field name, never an
    /// integer's value, so a run of them too large for one is the parser's to report.
    #[test]
    fn digits_after_a_dot_are_never_too_large() {
        for src in [
            ".9223372036854775809",
            "t.99999999999999999999999",
            "a?.9223372036854775809",
            "t.\n; 99999999999999999999999",
        ] {
            assert!(
                matches!(kinds(src)[..], [.., TokenKind::Int(_), TokenKind::Eof]),
                "{src}"
            );
        }
    }

    #[test]
    fn literal_too_large() {
        assert_eq!(err("9223372036854775809"), (Code::IntegerTooLarge, 1, 1));
        assert_eq!(
            err("99999999999999999999999"),
            (Code::IntegerTooLarge, 1, 1)
        );
    }

    fn chars(src: &str) -> Vec<char> {
        kinds(src)
            .into_iter()
            .filter_map(|k| match k {
                TokenKind::Char(c) => Some(c),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn char_literals() {
        assert_eq!(
            chars(r#"'a' 'ä' '😀' '{' '}' '\{' '\}' '\'' '"'"#),
            ['a', 'ä', '😀', '{', '}', '{', '}', '\'', '"']
        );
    }

    #[test]
    fn escapes() {
        assert_eq!(
            chars(
                r#"'\n' '\t' '\r' '\0' '\\' '\"' '\'' '\u{E4}' '\u{1F600}' '\u{D7FF}' '\u{E000}' '\u{10FFFF}'"#
            ),
            [
                '\n',
                '\t',
                '\r',
                '\0',
                '\\',
                '"',
                '\'',
                'ä',
                '😀',
                '\u{D7FF}',
                '\u{E000}',
                '\u{10FFFF}'
            ]
        );
    }

    /// Each error is at its escape, and the lexer goes on after the literal. An escape error
    /// beats a literal that is not closed. The last column is the source text the error covers.
    #[test]
    fn escape_errors() {
        use Code::{MalformedUnicodeEscape, NotScalarValue, UnknownEscape};
        for (src, expected, text) in [
            (r"'\q'", (UnknownEscape, 1, 2), r"\q"),
            ("'\\q\n", (UnknownEscape, 1, 2), r"\q"),
            ("'\\\u{200B}'", (UnknownEscape, 1, 2), "\\\u{200B}"),
            (r"'\u{}'", (MalformedUnicodeEscape, 1, 2), r"\u{}"),
            (
                r"'\u{1234567}'",
                (MalformedUnicodeEscape, 1, 2),
                r"\u{1234567}",
            ),
            (r"'\u{12'", (MalformedUnicodeEscape, 1, 2), r"\u{12"),
            (r"'\uE4'", (MalformedUnicodeEscape, 1, 2), r"\u"),
            (r"'\u{12G4}'", (MalformedUnicodeEscape, 1, 2), r"\u{12"),
            (r"'\u{D800}'", (NotScalarValue, 1, 2), r"\u{D800}"),
            (r"'\u{DFFF}'", (NotScalarValue, 1, 2), r"\u{DFFF}"),
            (r"'\u{110000}'", (NotScalarValue, 1, 2), r"\u{110000}"),
            (r"'ab\q'", (UnknownEscape, 1, 4), r"\q"),
        ] {
            let d = only(src);
            assert_eq!(at(src, &d), expected, "{src:?}");
            assert_eq!(&src[d.span().range()], text, "{src:?}");
        }
    }

    /// An unknown escape in a string literal gets an unsafe fix that assumes the `\` was meant as a
    /// backslash, and writes another one. In a `char` literal that would make two code points, so
    /// it gets none.
    #[test]
    fn unknown_escape_fix() {
        let d = only(r#""\q""#);
        assert_eq!(fix_edits(&d), Some((false, vec![(1, 1, "\\".to_string())])));
        assert_eq!(apply_fix(r#""\q""#, &d), r#""\\q""#);
        assert_eq!(fix_edits(&only(r"'\q'")), None);
    }

    #[test]
    fn char_literal_errors() {
        use Code::{LiteralNotClosed, NotOneCodePoint};
        for (src, expected) in [
            ("''", (NotOneCodePoint, 1, 1)),
            ("'ab'", (NotOneCodePoint, 1, 1)),
            ("'e\u{301}'", (NotOneCodePoint, 1, 1)),
            ("'a\n", (LiteralNotClosed, 1, 1)),
            ("'a", (LiteralNotClosed, 1, 1)),
            ("let c = '", (LiteralNotClosed, 1, 9)),
            ("let c = '\\", (LiteralNotClosed, 1, 9)),
            ("let c = '\\\n", (LiteralNotClosed, 1, 9)),
            ("let c = '\\\r\n", (LiteralNotClosed, 1, 9)),
        ] {
            assert_eq!(err(src), expected, "{src:?}");
        }
    }

    /// The error covers the whole literal, and the next token starts after it.
    #[test]
    fn char_literal_error_recovery() {
        let src = "'ab' 7";
        let lexed = lex(src).expect("the lexer recovers");
        let span = lexed.diagnostics[0].span();
        assert_eq!((span.start(), span.end()), (0, 4));
        assert_eq!(lexed.tokens[1].kind, int(7));
        let src = "'a 7\nx";
        let lexed = lex(src).expect("the lexer recovers");
        let span = lexed.diagnostics[0].span();
        assert_eq!((span.start(), span.end()), (0, 1));
        assert_eq!(lexed.tokens[1].kind, TokenKind::Newline);
    }

    /// Without a quote, brace or comment in its text, the literal gets an unsafe fix that turns it
    /// into a string.
    #[test]
    fn not_one_code_point_fix() {
        for (src, fixed) in [
            ("'ab'", r#""ab""#),
            ("''", r#""""#),
            ("'e\u{301}'", "\"e\u{301}\""),
            (r"'a\tb'", r#""a\tb""#),
        ] {
            let d = only(src);
            assert_eq!(fix_edits(&d).map(|(safe, _)| safe), Some(false), "{src}");
            assert_eq!(apply_fix(src, &d), fixed);
        }
        for src in ["'a{'", "'a}'", r#"'a"'"#, r#"'a\"'"#] {
            assert_eq!(fix_edits(&only(src)), None, "{src}");
        }
    }

    fn str(s: &str) -> TokenKind {
        TokenKind::Str(s.to_string())
    }

    #[test]
    fn string_literals() {
        assert_eq!(
            kinds(r#""" "ab" "ä😀" "\{\}" "a\0b""#),
            [
                str(""),
                str("ab"),
                str("ä😀"),
                str("{}"),
                str("a\0b"),
                TokenKind::Eof
            ]
        );
        // The text after an empty interpolation decodes on.
        let lexed = lex("\"a{ }b\"").expect("the lexer recovers");
        assert_eq!(lexed.tokens[0].kind, str("ab"));
    }

    /// A string reports an escape error as a `char` literal does, and only that error.
    #[test]
    fn string_escape_errors() {
        for (src, expected) in [
            (r#""\q""#, (Code::UnknownEscape, 1, 2)),
            (r#""\u{D800}""#, (Code::NotScalarValue, 1, 2)),
        ] {
            assert_eq!(err(src), expected, "{src}");
        }
        // The backslash unsafe fix works in a string too.
        let src = r#"print("a\q")"#;
        assert_eq!(apply_fix(src, &only(src)), r#"print("a\\q")"#);
    }

    /// Each error is the literal's only one; the last column is the source text it covers.
    #[test]
    fn string_errors() {
        use Code::{
            BraceInString, CommentInInterpolation, EmptyInterpolation, LiteralNotClosed,
            UnknownEscape,
        };
        for (src, expected, text) in [
            ("\"ab\n", (LiteralNotClosed, 1, 1), "\""),
            ("\"ab", (LiteralNotClosed, 1, 1), "\""),
            ("let s = \"\\\n", (LiteralNotClosed, 1, 9), "\""),
            ("let s = \"\\", (LiteralNotClosed, 1, 9), "\""),
            ("let s = \"\\\r\n", (LiteralNotClosed, 1, 9), "\""),
            (r#""a}b""#, (BraceInString, 1, 3), "}"),
            (r#""{x}}""#, (BraceInString, 1, 5), "}"),
            (r#""{ "}" }""#, (BraceInString, 1, 5), "}"),
            (r#""{}""#, (EmptyInterpolation, 1, 2), "{}"),
            (r#""{ }""#, (EmptyInterpolation, 1, 2), "{ }"),
            ("\"{\r}\"", (EmptyInterpolation, 1, 2), "{\r}"),
            ("\"{ \t}\"", (EmptyInterpolation, 1, 2), "{ \t}"),
            // A spec with no expression before it.
            (r#""{:>8}""#, (EmptyInterpolation, 1, 2), "{:>8}"),
            (r#""a{ :>{w}}""#, (EmptyInterpolation, 1, 3), "{ :>{w}}"),
            ("\"{:>8\n", (LiteralNotClosed, 1, 1), "\""),
            ("\"{:>8\"\n", (LiteralNotClosed, 1, 1), "\""),
            (r#""{x // c}""#, (CommentInInterpolation, 1, 5), "//"),
            ("\"{x\n", (LiteralNotClosed, 1, 1), "\""),
            ("\"{x", (LiteralNotClosed, 1, 1), "\""),
            ("\"a{x}b\n", (LiteralNotClosed, 1, 1), "\""),
            ("\"{ \"a\n", (LiteralNotClosed, 1, 4), "\""),
            ("\"{ \"{x\n", (LiteralNotClosed, 1, 4), "\""),
            // Brackets opened in an interpolation close with it, so lexing goes on.
            (
                "\"a{ [x // c\nlet t = 1",
                (CommentInInterpolation, 1, 8),
                "//",
            ),
            // A brace error yields to the end the literal lacks; an escape error does not.
            ("\"a}b\n", (LiteralNotClosed, 1, 1), "\""),
            ("\"{ }\n", (LiteralNotClosed, 1, 1), "\""),
            ("\"a}{x\n", (LiteralNotClosed, 1, 1), "\""),
            ("\"a}{ \"b\n", (LiteralNotClosed, 1, 6), "\""),
            ("\"a}{x // c}\"", (CommentInInterpolation, 1, 7), "//"),
            ("\"\\q\n", (UnknownEscape, 1, 2), "\\q"),
            ("\"\\q{x\n", (UnknownEscape, 1, 2), "\\q"),
        ] {
            let d = only(src);
            assert_eq!(at(src, &d), expected, "{src:?}");
            assert_eq!(&src[d.span().range()], text, "{src:?}");
        }
        // A literal that is not closed keeps the errors before it.
        assert_eq!(
            found("\"a}b\" \"c\n"),
            [(BraceInString, 1, 3), (LiteralNotClosed, 1, 7)]
        );
        assert_eq!(
            found("$ \"c\n"),
            [(Code::UnexpectedCharacter, 1, 1), (LiteralNotClosed, 1, 3)]
        );
    }

    /// A `}` gets a safe fix and an empty interpolation an unsafe fix; the others have none.
    #[test]
    fn string_error_fixes() {
        for (src, safe, fixed) in [
            (r#""a}b""#, true, r#""a\}b""#),
            (r#""{}""#, false, r#""\{\}""#),
            (r#""{ }""#, false, r#""\{ \}""#),
        ] {
            let d = only(src);
            assert_eq!(fix_edits(&d).map(|(c, _)| c), Some(safe), "{src}");
            assert_eq!(apply_fix(src, &d), fixed);
        }
        for src in ["\"ab\n", r#""{x // c}""#] {
            assert_eq!(fix_edits(&only(src)), None, "{src:?}");
        }
    }

    #[test]
    fn interpolation_tokens() {
        use TokenKind::{Eof, LParen, RParen, StrHead, StrMiddle, StrTail};
        assert_eq!(
            kinds(r#""a{x}b{ f(1) }c""#),
            [
                StrHead("a".to_string()),
                ident("x"),
                StrMiddle("b".to_string()),
                ident("f"),
                LParen,
                int(1),
                RParen,
                StrTail("c".to_string()),
                Eof
            ]
        );
    }

    /// Braces inside literals and blocks of an interpolation do not end it.
    #[test]
    fn interpolation_nesting() {
        use TokenKind::{Eof, EqEq, LBrace, LParen, RBrace, RParen, StrHead, StrTail};
        let empty = String::new;
        for (src, expected) in [
            (
                r#""{ "\}" }""#,
                vec![StrHead(empty()), str("}"), StrTail(empty()), Eof],
            ),
            (
                r#""{ c == '}' }""#,
                vec![
                    StrHead(empty()),
                    ident("c"),
                    EqEq,
                    TokenKind::Char('}'),
                    StrTail(empty()),
                    Eof,
                ],
            ),
            (
                r#""{ g({1}) }""#,
                vec![
                    StrHead(empty()),
                    ident("g"),
                    LParen,
                    LBrace,
                    int(1),
                    RBrace,
                    RParen,
                    StrTail(empty()),
                    Eof,
                ],
            ),
            (
                r#""{ "{x}" }""#,
                vec![
                    StrHead(empty()),
                    StrHead(empty()),
                    ident("x"),
                    StrTail(empty()),
                    StrTail(empty()),
                    Eof,
                ],
            ),
        ] {
            assert_eq!(kinds(src), expected, "{src}");
        }
    }

    /// A `:` that no bracket inside the interpolation encloses starts a format spec, which runs
    /// to the `}` that ends the interpolation; a `{` in it opens a pair whose `}` is its text.
    #[test]
    fn format_spec_tokens() {
        use TokenKind::{Colon, Eof, FormatSpec, LParen, RParen, StrHead, StrTail};
        let empty = String::new;
        for (src, expected) in [
            (
                r#""{x:>8.2}""#,
                vec![
                    StrHead(empty()),
                    ident("x"),
                    FormatSpec(">8.2".to_string()),
                    StrTail(empty()),
                    Eof,
                ],
            ),
            (
                r#""{x:>{w}}""#,
                vec![
                    StrHead(empty()),
                    ident("x"),
                    FormatSpec(">{w}".to_string()),
                    StrTail(empty()),
                    Eof,
                ],
            ),
            (
                r#""{"a":>4}""#,
                vec![
                    StrHead(empty()),
                    TokenKind::Str("a".to_string()),
                    FormatSpec(">4".to_string()),
                    StrTail(empty()),
                    Eof,
                ],
            ),
            (
                r#""{f(a: 1)}""#,
                vec![
                    StrHead(empty()),
                    ident("f"),
                    LParen,
                    ident("a"),
                    Colon,
                    int(1),
                    RParen,
                    StrTail(empty()),
                    Eof,
                ],
            ),
            (
                r#""{':'}""#,
                vec![
                    StrHead(empty()),
                    TokenKind::Char(':'),
                    StrTail(empty()),
                    Eof,
                ],
            ),
        ] {
            assert_eq!(kinds(src), expected, "{src}");
        }
        // The spec's span starts at its `:`.
        let src = r#""é{x:<3}""#;
        let spec = &tokens(src)[2];
        assert_eq!(&src[spec.span.range()], ":<3");
    }

    /// A spec that a `"` or the line end cuts off leaves the literal unclosed, and that is the
    /// one error: the rest of the line is lost in the literal.
    #[test]
    fn format_spec_cut_off() {
        for (src, expected) in [
            (
                "let s = \"{x:>8\"\nlet t = 1",
                (Code::LiteralNotClosed, 1, 9),
            ),
            (
                "let s = \"{x:+\" + \"abc}\"\nlet t = 1",
                (Code::LiteralNotClosed, 1, 9),
            ),
            (
                "let s = \"{ \"{x:>{w\" }\"\nlet t = 1",
                (Code::LiteralNotClosed, 1, 12),
            ),
            ("let s = \"{x:>8\nlet t = 1", (Code::LiteralNotClosed, 1, 9)),
            ("let s = \"{x:>8", (Code::LiteralNotClosed, 1, 9)),
        ] {
            assert_eq!(found(src), [expected], "{src:?}");
        }
    }

    /// A `}` or other closer in an interpolation cannot close past a bracket opened in it, nor
    /// close one opened before it; the error points at the closer and the bracket left open.
    #[test]
    fn interpolation_confines_brackets() {
        for (src, closer, opened) in [
            ("fn main() {\n    print(\"{ f( }\")\n}", (2, 17), (2, 15)),
            ("print(\"{x)}\")", (1, 10), (1, 8)),
            ("let s = \"a{ (x }\"", (1, 16), (1, 13)),
        ] {
            let d = fatal(src);
            assert_eq!(
                at(src, &d),
                (Code::UnexpectedClosingBracket, closer.0, closer.1),
                "{src}"
            );
            let [(open, _)] = d.secondary() else {
                panic!("one secondary span, got {:?}", d.secondary());
            };
            assert_eq!(line_col(src, open.start()), opened, "{src}");
            assert_eq!(open.end(), open.start() + 1, "{src}");
        }
    }

    /// A literal that is not closed is one `Str` token, whichever segment its line end cuts, so
    /// no bracket opened in it stays open, and the line break after it ends its statement.
    #[test]
    fn unclosed_literal_is_one_str_token() {
        use TokenKind::{Eof, Eq, Let, Newline};
        for (literal, text) in [
            ("\"ab", "ab"),
            ("\"a{ f(x", ""),
            ("\"a{x}b", ""),
            ("\"a{ \"b", ""),
            ("\"a{x}b{ \"c{y}d", ""),
        ] {
            let src = format!("let s = {literal}\nlet t = 1");
            let lexed = lex(&src).expect("the lexer recovers");
            let kinds: Vec<_> = lexed.tokens.into_iter().map(|t| t.kind).collect();
            assert_eq!(
                kinds,
                [
                    Let,
                    ident("s"),
                    Eq,
                    str(text),
                    Newline,
                    Let,
                    ident("t"),
                    Eq,
                    int(1),
                    Newline,
                    Eof
                ],
                "{literal}"
            );
        }
    }

    /// `.config/nextest.toml` kills this test after 30 s, matching it by name: a rename must
    /// update the override there. Each unclosed literal once rescanned every earlier diagnostic.
    #[test]
    fn many_unclosed_strings_lex_in_linear_time() {
        const LINES: usize = 500_000;
        let src = "\"x\n".repeat(LINES);
        let lexed = lex(&src).expect("the lexer recovers");
        assert_eq!(lexed.diagnostics.len(), LINES);
    }

    #[test]
    fn string_ends_statement() {
        use TokenKind::{Eof, Eq, LParen, Let, Newline, RParen, StrHead, StrTail};
        assert_eq!(
            all_kinds("let s = \"a\"\nlet t = \"{s}\"\nprint(t)"),
            [
                Let,
                ident("s"),
                Eq,
                str("a"),
                Newline,
                Let,
                ident("t"),
                Eq,
                StrHead(String::new()),
                ident("s"),
                StrTail(String::new()),
                Newline,
                ident("print"),
                LParen,
                ident("t"),
                RParen,
                Newline,
                Eof
            ]
        );
    }

    #[test]
    fn char_ends_statement() {
        assert_eq!(
            all_kinds("let c = 'a'\nprint(c)"),
            [
                TokenKind::Let,
                ident("c"),
                TokenKind::Eq,
                TokenKind::Char('a'),
                TokenKind::Newline,
                ident("print"),
                TokenKind::LParen,
                ident("c"),
                TokenKind::RParen,
                TokenKind::Newline,
                TokenKind::Eof
            ]
        );
    }

    /// Inside brackets no `Newline` follows a char literal.
    #[test]
    fn char_in_brackets_has_no_newline() {
        use TokenKind::{Comma, Eof, LBracket, LParen, Newline, RBracket, RParen};
        assert_eq!(
            all_kinds("print('a'\n)"),
            [
                ident("print"),
                LParen,
                TokenKind::Char('a'),
                RParen,
                Newline,
                Eof
            ]
        );
        assert_eq!(
            all_kinds("['a'\n, 'b']"),
            [
                LBracket,
                TokenKind::Char('a'),
                Comma,
                TokenKind::Char('b'),
                RBracket,
                Newline,
                Eof
            ]
        );
    }

    #[test]
    fn literal_not_closed_help() {
        // A `\r\n` stops a literal as `\n` does and keeps the line-break help; the end of the
        // file gives none.
        for (src, help) in [
            ("let s = \"a\r\n", true),
            ("let s = \"a\n", true),
            ("let s = \"a", false),
        ] {
            let d = only(src);
            assert_eq!(d.code(), Code::LiteralNotClosed, "{src:?}");
            assert_eq!(d.help().is_some(), help, "{src:?}");
        }
    }

    #[test]
    fn unexpected_character_fix() {
        // A lone `&` gets an unsafe fix to the doubled operator; a lone `|` is the parser's.
        let src = "a & b";
        let d = only(src);
        assert_eq!(d.code(), Code::UnexpectedCharacter, "{src}");
        assert_eq!(line_col(src, d.span().start()), (1, 3), "{src}");
        assert_eq!(
            fix_edits(&d),
            Some((false, vec![(2, 3, "&&".to_string())])),
            "{src}"
        );
        // No fix is offered for any other character.
        let d = only("a $ b");
        assert_eq!(d.code(), Code::UnexpectedCharacter);
        assert_eq!(fix_edits(&d), None);
        assert_eq!(d.help(), None);
    }

    #[test]
    fn unknown_char_counts_chars() {
        let src = "// ä\nlet é = 1";
        let d = only(src);
        assert_eq!(d.code(), Code::UnexpectedCharacter);
        assert_eq!(line_col(src, d.span().start()), (2, 5));
        // The span covers the whole two-byte char, so rendering does not panic.
        assert!(
            d.render("t.sisu", src, &LineIndex::new(src), false)
                .contains("2:5")
        );
    }

    #[test]
    fn comment_skipped() {
        assert_eq!(kinds("x // päivää"), [ident("x"), TokenKind::Eof]);
    }

    #[test]
    fn token_spans_are_byte_offsets() {
        // The two-byte `ä` shifts byte offsets away from char offsets.
        let spans: Vec<Span> = tokens("// ä\nab 12 +=")
            .into_iter()
            .map(|t| t.span)
            .collect();
        assert_eq!(
            spans,
            [
                Span::new(6, 8),
                Span::new(9, 11),
                Span::new(12, 14),
                Span::new(14, 14)
            ]
        );
    }

    #[test]
    fn continuation_spans_skip_the_gap() {
        let spans = |src| -> Vec<Span> { tokens(src).into_iter().map(|t| t.span).collect() };
        assert_eq!(
            spans("a\n  .b"),
            [
                Span::new(0, 1),
                Span::new(4, 5),
                Span::new(5, 6),
                Span::new(6, 6),
                Span::new(6, 6)
            ]
        );
        assert_eq!(
            spans("a\n// c\n  .b"),
            [
                Span::new(0, 1),
                Span::new(9, 10),
                Span::new(10, 11),
                Span::new(11, 11),
                Span::new(11, 11)
            ]
        );
    }

    /// `.config/nextest.toml` kills this test after 30 s, matching it by name: a rename must
    /// update the override there.
    #[test]
    fn long_gaps_lex_correctly() {
        use TokenKind::{Dot, Eof, Newline};
        const LINES: usize = 200_000;
        for gap in ["\n", "\n// c"] {
            for (tail, expected) in [
                ("b", vec![ident("a"), Newline, ident("b"), Newline, Eof]),
                (".b", vec![ident("a"), Dot, ident("b"), Newline, Eof]),
            ] {
                let src = format!("a{}\n{tail}", gap.repeat(LINES));
                assert_eq!(all_kinds(&src), expected, "gap {gap:?} then {tail}");
            }
            // A continuation, then another long gap before a new statement.
            let src = format!("a{0}\n.b{0}\nc", gap.repeat(LINES));
            let expected = [
                ident("a"),
                Dot,
                ident("b"),
                Newline,
                ident("c"),
                Newline,
                Eof,
            ];
            assert_eq!(all_kinds(&src), expected, "gap {gap:?}, continuation, gap");
        }
    }

    #[test]
    fn unicode_space_in_the_gap_is_unexpected() {
        let src = "a\n\u{00a0}.b";
        assert_eq!(err(src), (Code::UnexpectedCharacter, 2, 1));
    }

    /// The `lex_*` fixtures pin how each is named.
    #[test]
    fn invisible_and_visible_characters_are_unexpected() {
        for c in [
            '\u{00a0}',
            '\u{2028}',
            '\u{0085}',
            '\u{0001}',
            '\u{007f}',
            '\u{00ad}',
            '\u{200b}',
            '\u{202e}',
            '\u{feff}',
            '\u{e0020}',
            '$',
            '\u{e9}',
            '\u{3042}',
            '\u{1f600}',
        ] {
            let src = format!("a {c}");
            assert_eq!(err(&src), (Code::UnexpectedCharacter, 1, 3), "{c:?}");
        }
    }

    #[test]
    fn byte_order_mark_is_removed_only_at_the_start_of_the_file() {
        let src = "\u{feff}fn main() {}";
        let d = only(src);
        assert_eq!(at(src, &d), (Code::UnexpectedCharacter, 1, 1));
        assert_eq!(fix_edits(&d), Some((true, vec![(0, 3, String::new())])));
        joined_parses(src, &d);
        for src in ["fn\u{feff}", "\n\u{feff}"] {
            assert_eq!(fix_edits(&only(src)), None, "{src:?}");
        }
    }

    fn all_kinds(src: &str) -> Vec<TokenKind> {
        tokens(src).into_iter().map(|t| t.kind).collect()
    }

    /// A line-end `>` ends the statement only where it closes a type annotation, and the scan
    /// back for the annotation steps over brackets, stops at its block's `{` and skips the `;`s.
    #[test]
    fn a_line_end_gt_should_end_only_a_type_annotation() {
        let newlines = |src: &str| {
            all_kinds(src)
                .iter()
                .filter(|k| **k == TokenKind::Newline)
                .count()
        };
        assert_eq!(newlines("class D {\n    b: Box<[i64]>\n    n: i64\n}"), 3);
        assert_eq!(newlines("class D {\n    b: Box<i64>;\n    n: i64\n}"), 3);
        assert_eq!(newlines("fn f() -> bool {\n    x < y >\n        z\n}"), 2);
    }

    /// A loop label's `:` does not start a type annotation, so a `>` at a line's end after it still
    /// compares and the condition goes on.
    #[test]
    fn a_loop_label_should_not_start_a_type_annotation() {
        let newlines = |src: &str| {
            all_kinds(src)
                .iter()
                .filter(|k| **k == TokenKind::Newline)
                .count()
        };
        let src =
            "fn f() {\n    outer: while a < b && c >\n        d {\n        break outer\n    }\n}";
        assert_eq!(newlines(src), 3);
    }

    /// An interpolated string is one bracket to the statement around it: after it closes, a `{`
    /// that starts a line after a `>` asks that statement, not the one outside it nor the one in
    /// the bracket the string stood in.
    #[test]
    fn an_interpolated_string_should_close_like_a_bracket() {
        let braces = |src: &str| {
            let lexed = lex(src).expect("the source should lex");
            lexed
                .diagnostics
                .iter()
                .filter(|d| d.code() == Code::BraceOnNewLine)
                .count()
        };
        assert_eq!(braces("fn f() {\n    b = (\"{x}\") >\n        { 1 }\n}"), 0);
        assert_eq!(
            braces("fn f(x: Box<i64> = { \"{y}\" }, y: Box<i64>\n{}) {}"),
            1
        );
    }

    #[test]
    fn newline_after_rparen() {
        use TokenKind::{Eof, LParen, Newline, RParen};
        assert_eq!(
            all_kinds("f()\nx"),
            [
                ident("f"),
                LParen,
                RParen,
                Newline,
                ident("x"),
                Newline,
                Eof
            ]
        );
    }

    #[test]
    fn no_newline_inside_parens() {
        use TokenKind::{Eof, LParen, Newline, Plus, RParen};
        assert_eq!(
            all_kinds("f(1\n+2)"),
            [
                ident("f"),
                LParen,
                int(1),
                Plus,
                int(2),
                RParen,
                Newline,
                Eof
            ]
        );
    }

    #[test]
    fn block_inside_parens() {
        use TokenKind::{Eof, LBrace, LParen, Newline, RBrace, RParen};
        assert_eq!(
            all_kinds("(\n{\nx\ny\n}\n)"),
            [
                LParen,
                LBrace,
                ident("x"),
                Newline,
                ident("y"),
                Newline,
                RBrace,
                RParen,
                Newline,
                Eof
            ]
        );
    }

    #[test]
    fn newline_after_comment() {
        use TokenKind::{Eof, Newline};
        assert_eq!(
            all_kinds("x // hi\ny"),
            [ident("x"), Newline, ident("y"), Newline, Eof]
        );
    }

    #[test]
    fn no_newline_after_operator() {
        use TokenKind::{Eof, Newline, Plus};
        assert_eq!(all_kinds("1 +\n2"), [int(1), Plus, int(2), Newline, Eof]);
    }

    #[test]
    fn blank_lines_give_one_newline() {
        use TokenKind::{Eof, Newline};
        assert_eq!(
            all_kinds("x\n\n\ny"),
            [ident("x"), Newline, ident("y"), Newline, Eof]
        );
    }

    #[test]
    fn newline_at_eof() {
        use TokenKind::{Eof, Newline};
        assert_eq!(all_kinds("x"), [ident("x"), Newline, Eof]);
    }

    #[test]
    fn crlf_line_endings() {
        use TokenKind::{Eof, Newline};
        assert_eq!(
            all_kinds("x\r\ny\r\n"),
            [ident("x"), Newline, ident("y"), Newline, Eof]
        );
    }

    #[test]
    fn newline_and_eof_spans() {
        // A Newline sits empty at the line end, before `\r\n` or `\n`, or at end of file;
        // Eof sits just after the last code.
        let spans = |src| -> Vec<Span> { tokens(src).into_iter().map(|t| t.span).collect() };
        assert_eq!(
            spans("x // c\n"),
            [Span::new(0, 1), Span::new(6, 6), Span::new(1, 1)]
        );
        assert_eq!(
            spans("x\r\n"),
            [Span::new(0, 1), Span::new(1, 1), Span::new(1, 1)]
        );
        assert_eq!(
            spans("x"),
            [Span::new(0, 1), Span::new(1, 1), Span::new(1, 1)]
        );
    }

    #[test]
    fn unary_minus_line_is_fine() {
        use TokenKind::{Eof, Minus, Newline};
        assert_eq!(
            all_kinds("x\n-y"),
            [ident("x"), Newline, Minus, ident("y"), Newline, Eof]
        );
    }

    #[test]
    fn binary_minus_line_is_error() {
        use TokenKind::{Eof, Eq, Let, Minus, Newline};
        let src = "let t = a\n- b";
        assert_eq!(err(src), (Code::LeadingBinaryMinus, 2, 1));
        // The `-` is kept as a `Minus`.
        let kinds: Vec<_> = lex(src)
            .expect("the lexer recovers")
            .tokens
            .into_iter()
            .map(|t| t.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                Let,
                ident("t"),
                Eq,
                ident("a"),
                Newline,
                Minus,
                ident("b"),
                Newline,
                Eof
            ]
        );
    }

    #[test]
    fn unexpected_close() {
        assert_eq!(
            at("x)", &fatal("x)")),
            (Code::UnexpectedClosingBracket, 1, 2)
        );
    }

    #[test]
    fn mismatched_close() {
        let d = fatal("(}");
        let [(open, _)] = d.secondary() else {
            panic!("one secondary span, got {:?}", d.secondary());
        };
        assert_eq!(line_col("(}", open.start()), (1, 1));
        // With nothing open, there is nothing to point at.
        let d = fatal("}");
        assert_eq!(d.secondary(), []);
    }

    #[test]
    fn unclosed_paren() {
        assert_eq!(at("(x", &fatal("(x")), (Code::UnclosedBracket, 1, 1));
    }

    #[test]
    fn binary_minus_inside_parens_is_fine() {
        use TokenKind::{Eof, LParen, Minus, Newline, RParen};
        assert_eq!(
            all_kinds("(a\n- b)"),
            [LParen, ident("a"), Minus, ident("b"), RParen, Newline, Eof]
        );
    }

    #[test]
    fn binary_minus_after_operator_is_fine() {
        use TokenKind::{Eof, Minus, Newline};
        assert_eq!(
            all_kinds("a -\n- b"),
            [ident("a"), Minus, Minus, ident("b"), Newline, Eof]
        );
    }

    #[test]
    fn bracket_tokens_and_keywords() {
        use TokenKind::{DotDot, Eof, For, In, LBracket, RBracket, Semicolon};
        assert_eq!(
            kinds("[1, 2][0]"),
            [
                LBracket,
                int(1),
                TokenKind::Comma,
                int(2),
                RBracket,
                LBracket,
                int(0),
                RBracket,
                Eof
            ]
        );
        assert_eq!(
            kinds("[0; 3]"),
            [LBracket, int(0), Semicolon, int(3), RBracket, Eof]
        );
        assert_eq!(kinds("for x in"), [For, ident("x"), In, Eof]);
        assert_eq!(kinds("0..n"), [int(0), DotDot, ident("n"), Eof]);
        assert_eq!(kinds("a.b"), [ident("a"), TokenKind::Dot, ident("b"), Eof]);
    }

    #[test]
    fn brackets_suppress_newlines() {
        use TokenKind::{Comma, Eof, LBracket, Newline, RBracket};
        assert_eq!(
            all_kinds("[\n1,\n2\n]"),
            [LBracket, int(1), Comma, int(2), RBracket, Newline, Eof]
        );
        // The only `Newline` is the one after the closing `]`.
        let nested = all_kinds("[\n    [1],\n    [2],\n]");
        assert_eq!(nested.iter().filter(|k| **k == Newline).count(), 1);
        assert_eq!(nested[nested.len() - 2], Newline);
    }

    #[test]
    fn block_inside_brackets_separates_statements() {
        let kinds = all_kinds("[\n    if c {\n        let n = 1\n        n\n    } else { 0 },\n]");
        let one = kinds.iter().position(|k| *k == int(1));
        assert_eq!(
            one.map(|i| &kinds[i + 1]),
            Some(&TokenKind::Newline),
            "a `Newline` should follow `1` inside the block"
        );
    }

    #[test]
    fn bracket_line_start_outside_brackets_is_a_new_statement() {
        use TokenKind::{Eof, LBracket, Newline, RBracket};
        assert_eq!(
            all_kinds("a\n[0]"),
            [
                ident("a"),
                Newline,
                LBracket,
                int(0),
                RBracket,
                Newline,
                Eof
            ]
        );
    }

    #[test]
    fn range_is_not_a_continuation() {
        use TokenKind::{DotDot, Eof, Newline};
        assert_eq!(
            all_kinds("a\n..b"),
            [ident("a"), Newline, DotDot, ident("b"), Newline, Eof]
        );
    }

    #[test]
    fn bracket_on_new_line_inside_brackets() {
        for src in ["fn main() { (a\n[0]) }", "fn main() { [a\n[0]] }"] {
            let d = only(src);
            assert_eq!(d.code(), Code::BracketOnNewLine, "{src}");
            assert_eq!(d.span(), Span::new(15, 16), "{src}");
            assert!(keeps_token(src, &TokenKind::LBracket, d.span()), "{src}");
        }
    }

    /// Whether the tokens of `src` hold a `kind` at `span`.
    fn keeps_token(src: &str, kind: &TokenKind, span: Span) -> bool {
        let lexed = lex(src).expect("the lexer recovers");
        lexed
            .tokens
            .iter()
            .any(|t| t.kind == *kind && t.span == span)
    }

    #[test]
    fn else_on_new_line_inside_brackets() {
        assert_eq!(
            err("fn main() { [if c { 1 }\nelse { 2 }] }"),
            (Code::ElseOnNewLine, 2, 1)
        );
    }

    #[test]
    fn else_on_new_line_inside_parens() {
        assert_eq!(
            err("fn main() { print(if true { 1 }\nelse { 2 }) }"),
            (Code::ElseOnNewLine, 2, 1)
        );
    }

    #[test]
    fn else_on_new_line_fix() {
        // The gap after the `}` becomes one space; the lexer cannot tell the `if` has no `else`.
        let src = "fn main() { print(if true { 1 }\nelse { 2 }) }";
        let d = only(src);
        assert_eq!(
            fix_edits(&d),
            Some((false, vec![(31, 32, " ".to_string())]))
        );
        assert!(keeps_token(src, &TokenKind::Else, d.span()), "{src}");
        // Only a `}` before the `else` can take it.
        let d = only("fn main() { f(x\nelse y) }");
        assert_eq!(d.code(), Code::ElseOnNewLine);
        assert_eq!(fix_edits(&d), None);
    }

    #[test]
    fn brace_on_new_line_inside_parens() {
        assert_eq!(
            err("fn main() { print(if true\n{ 1 } else { 2 }) }"),
            (Code::BraceOnNewLine, 2, 1)
        );
    }

    #[test]
    fn brace_on_new_line_after_else() {
        assert_eq!(
            err("fn main() {\n    if true { print(1) } else\n    { print(2) }\n}"),
            (Code::BraceOnNewLine, 3, 5)
        );
    }

    #[test]
    fn brace_on_new_line_after_else_inside_parens() {
        assert_eq!(
            err("fn main() { print(if true { 1 } else\n{ 2 }) }"),
            (Code::BraceOnNewLine, 2, 1)
        );
    }

    /// Applies the one fix of `d` to `src` and asserts the result lexes and parses.
    fn joined_parses(src: &str, d: &Diagnostic) {
        let fixed = apply_fix(src, d);
        crate::parser::parse_ok(&fixed);
    }

    #[test]
    fn brace_on_new_line_fix() {
        // Every header keyword makes the fix safe, and the gap before the `{` becomes a space.
        for (src, gap, at) in [
            ("fn main()\n{\n}", (9, 10), (2, 1)),
            ("class A\n{\n}", (7, 8), (2, 1)),
            ("enum A\n{\n}", (6, 7), (2, 1)),
            (
                "fn main() {\n    match x\n    {\n    }\n}",
                (23, 28),
                (3, 5),
            ),
            (
                "fn main() {\n    if true\n    {\n    }\n}",
                (23, 28),
                (3, 5),
            ),
            (
                "fn main() {\n    while true\n    {\n    }\n}",
                (26, 31),
                (3, 5),
            ),
            (
                "fn main() {\n    for i in xs\n    {\n    }\n}",
                (27, 32),
                (3, 5),
            ),
            (
                "fn main() {\n    if a {} else\n    {\n    }\n}",
                (28, 33),
                (3, 5),
            ),
            // A `let` inside a closed block is not on the header's line.
            (
                "fn main() {\n    if c { let y = 1 } else\n    {\n    }\n}",
                (39, 44),
                (3, 5),
            ),
        ] {
            let d = only(src);
            assert_eq!(d.code(), Code::BraceOnNewLine, "{src}");
            assert_eq!(line_col(src, d.span().start()), at, "{src}");
            assert_eq!(
                fix_edits(&d),
                Some((true, vec![(gap.0, gap.1, " ".to_string())])),
                "{src}"
            );
            joined_parses(src, &d);
        }
    }

    #[test]
    fn brace_on_new_line_is_unsafe_over_a_comment() {
        // The join would delete the comment.
        let src = "fn main() {\n    if c // check it\n    {\n    }\n}";
        let d = only(src);
        assert_eq!(
            (d.code(), line_col(src, d.span().start())),
            (Code::BraceOnNewLine, (3, 5))
        );
        assert!(fix_edits(&d).is_some_and(|(safe, _)| !safe));
    }

    #[test]
    fn brace_on_new_line_is_unsafe_after_a_multi_line_header() {
        // The header keyword is on an earlier line of the same statement.
        for (src, at) in [
            (
                "fn main() {\n    if f(\n        1\n    )\n    {\n    }\n}",
                (5, 5),
            ),
            ("fn f(\n    x: i64\n)\n{\n}", (4, 1)),
            ("fn main() {\n    while\n        c\n    {\n    }\n}", (4, 5)),
            (
                "fn main() {\n    while a &&\n        b\n    {\n    }\n}",
                (4, 5),
            ),
            (
                "fn main() {\n    if xs\n        .is_empty()\n    {\n    }\n}",
                (4, 5),
            ),
        ] {
            let d = only(src);
            assert_eq!(d.code(), Code::BraceOnNewLine, "{src}");
            assert_eq!(line_col(src, d.span().start()), at, "{src}");
            assert!(
                fix_edits(&d).is_some_and(|(safe, _)| !safe),
                "{src}: {:?}",
                d.fixes()
            );
            joined_parses(src, &d);
        }
    }

    #[test]
    fn brace_on_new_line_has_no_fix() {
        for (src, at) in [
            // The previous statement is not a header: joining gives `print(1) {`.
            ("fn main() {\n    print(1)\n    {\n    }\n}", (3, 5)),
            ("fn main() {\n    x = 1\n    {\n    }\n}", (3, 5)),
            ("fn main() {\n    return 1\n    {\n    }\n}", (3, 5)),
            // A header keyword inside a statement that is not a header.
            ("fn main() {\n    let x = if c\n    {\n    }\n}", (3, 5)),
            (
                "fn main() {\n    x = if c { 1 } else\n    {\n    }\n}",
                (3, 5),
            ),
            // The statement starts after a block's `{`, not at the `if`.
            ("fn main() {\n    if c { x = 1\n    {\n    }\n}}", (3, 5)),
            // The previous line ends with `}`: joining gives `} {`.
            (
                "fn main() {\n    if c { print(1) }\n    {\n    }\n}",
                (3, 5),
            ),
            (
                "fn main() {\n    if true { while false }\n    {\n    }\n}",
                (3, 5),
            ),
            // A `}` after the keyword: the header has its block, or the keyword sits in a closed
            // one.
            (
                "fn main() {\n    if c { 1 } else { 2 } + 3\n    {\n    }\n}",
                (3, 5),
            ),
            (
                "fn main() {\n    if c { while c } == c\n    {\n    }\n}",
                (3, 5),
            ),
            // Inside `(` or `[`, the moved `{` would still sit in the unclosed bracket.
            ("fn main() {\n    if (c\n    { 1 }) {\n    }\n}", (3, 5)),
            (
                "fn main() {\n    for x in [1,\n        2\n    {\n    }]\n}",
                (4, 5),
            ),
        ] {
            let d = only(src);
            assert_eq!(d.code(), Code::BraceOnNewLine, "{src}");
            assert_eq!(line_col(src, d.span().start()), at, "{src}");
            assert_eq!(fix_edits(&d), None, "{src}");
        }
    }

    #[test]
    fn safe_brace_fix_removes_the_error() {
        let src = "fn main() {\n    for i in 0..3 // a loop\n    {\n        print(i)\n    }\n}\n";
        let d = only(src);
        let fixed = apply_fix(src, &d);
        assert_eq!(
            fixed,
            "fn main() {\n    for i in 0..3 {\n        print(i)\n    }\n}\n"
        );
        assert!(lex_clean(&fixed).is_ok(), "{fixed}");
    }

    #[test]
    fn semicolon_ends_a_line() {
        use TokenKind::{Eof, Newline, Semicolon};
        assert_eq!(
            all_kinds("x;\ny"),
            [ident("x"), Newline, ident("y"), Newline, Eof]
        );
        // Inside brackets a `;` separates the parts of an array.
        assert_eq!(
            all_kinds("[0;\n3]"),
            [
                TokenKind::LBracket,
                int(0),
                Semicolon,
                int(3),
                TokenKind::RBracket,
                Newline,
                Eof
            ]
        );
    }

    /// What the lexer makes of a `;` in statement context.
    #[derive(Debug, PartialEq)]
    enum Judged {
        /// A `Newline` over the `;`.
        Sep,
        /// No token, and a `RedundantSemicolon` at this line and column.
        Gone(usize, usize),
        /// A `Semicolon`, for the parser to report.
        Kept,
    }

    /// A `;` that starts its line takes the blanks after it, so the line keeps its indentation,
    /// whether one fix applies or all do; with code before it, the fix removes the `;` alone.
    #[test]
    fn redundant_semicolon_at_line_start_should_keep_indentation() {
        for (src, alone, all) in [
            ("x\n    ; y", vec!["x\n    y"], "x\n    y"),
            ("x\n\t; y", vec!["x\n\ty"], "x\n\ty"),
            ("x\n    ;y", vec!["x\n    y"], "x\n    y"),
            ("x\n    ;   \ny", vec!["x\n    \ny"], "x\n    \ny"),
            ("x\n    ;; y", vec!["x\n    ; y", "x\n    ;y"], "x\n    y"),
            ("x\n    ; ; y", vec!["x\n    ; y", "x\n    ; y"], "x\n    y"),
            ("x\n    ;;y", vec!["x\n    ;y", "x\n    ;y"], "x\n    y"),
            // Code before the `;` on its line: the fix removes the `;` alone.
            ("x\n    y;\n", vec!["x\n    y\n"], "x\n    y\n"),
        ] {
            let lexed = lex(src).expect("the lexer recovers");
            let ws: Vec<&Diagnostic> = (lexed.warnings.iter())
                .filter(|d| d.code() == Code::RedundantSemicolon)
                .collect();
            let each: Vec<String> = ws.iter().map(|d| apply_fix(src, d)).collect();
            assert_eq!(each, alone, "{src:?}");
            let edits: Vec<_> = (ws.iter())
                .flat_map(|d| d.fixes().iter().flat_map(crate::diagnostic::Fix::edits))
                .collect();
            assert_eq!(
                crate::diagnostic::explanations::apply(src, &edits),
                all,
                "{src:?}"
            );
        }
    }

    #[test]
    fn semicolon_separates_or_is_redundant() {
        use Judged::{Gone, Kept, Sep};
        for (src, want) in [
            ("x; y", vec![Sep]),
            ("x;\ny", vec![Gone(1, 2)]),
            ("{ x; }", vec![Gone(1, 4)]),
            ("{ ; x }", vec![Gone(1, 3)]),
            ("x\n; y", vec![Gone(2, 1)]),
            (";x", vec![Gone(1, 1)]),
            ("x;; y", vec![Sep, Gone(1, 3)]),
            ("x;", vec![Gone(1, 2)]),
            ("fn f() {}; fn g() {}", vec![Gone(1, 10)]),
            ("fn main(); { }", vec![Kept]),
            ("if c { a }; else { b }", vec![Gone(1, 11)]),
            ("const A = 1; const B = 2", vec![Sep]),
            ("const X = if c { 1 } else { 2 }; fn g() {}", vec![Sep]),
            // Only a top-level keyword starts an item: a `fn` in brackets or a `const` in a block
            // does not change what the `;` after the item is.
            ("const X = [fn]; fn g() {}", vec![Sep]),
            ("fn f() { const X = 1 }; fn g() {}", vec![Gone(1, 23)]),
            ("f({ a; b })", vec![Sep]),
            ("\"{ { a; b } }\"", vec![Sep]),
            ("f(a; b)", vec![Kept]),
            ("[0; 3]", vec![Kept]),
            ("\"{a; b}\"", vec![Kept]),
            // The first `;` makes the pass run, which must still leave the interpolation's alone.
            ("x; \"{a; b}\"", vec![Sep, Kept]),
            ("let x = ; y", vec![Kept]),
            ("let x =\n; y", vec![Gone(2, 1)]),
            ("x\n; .f()", vec![Kept]),
            // Without the `;`, `+` would take `.f()` across nothing: it cannot end a statement.
            ("x +; .f()", vec![Kept]),
            ("x;\n.f()", vec![Sep]),
            (
                "fn main() {\n    let x = 1;\n    {\n    }\n}",
                vec![Gone(2, 14)],
            ),
            (
                "fn main() {\n    let x = 1;\n    ; { }\n}",
                vec![Gone(2, 14), Gone(3, 5)],
            ),
        ] {
            let lexed = lex(src).expect("the lexer recovers");
            let judged: Vec<Judged> = (src.match_indices(';').map(|(i, _)| i))
                .map(|i| {
                    let token = lexed.tokens.iter().find(|t| t.span == Span::new(i, i + 1));
                    let warning = lexed.warnings.iter().find(|d| d.span().start() == i);
                    match (token.map(|t| &t.kind), warning.map(|d| at(src, d))) {
                        (Some(TokenKind::Newline), None) => Sep,
                        (None, Some((Code::RedundantSemicolon, line, col))) => Gone(line, col),
                        (Some(TokenKind::Semicolon), None) => Kept,
                        other => panic!("{src}: the `;` at {i} is {other:?}"),
                    }
                })
                .collect();
            assert_eq!(judged, want, "{src}");
            let gone = want.iter().filter(|j| matches!(j, Gone(..))).count();
            assert_eq!(lexed.warnings.len(), gone, "{src}");
        }
    }

    #[test]
    fn semicolon_before_a_line_start_lexes_clean() {
        // The `[` after `;` starts an array element.
        assert!(lex_clean("fn main() { let b = [0;\n[2][0]] }").is_ok());
    }

    /// The token kinds of `src`, which the lexer recovers from.
    fn recovered_kinds(src: &str) -> Vec<TokenKind> {
        let lexed = lex(src).expect("the lexer recovers");
        lexed.tokens.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn layout_looks_past_a_semicolon() {
        use Code::{BraceOnNewLine, LeadingBinaryMinus, UnexpectedCharacter};
        for (src, want) in [
            // A `;` ends the line above as a line break does, so the `{` below it is on its own
            // line, and so is a `{` after a line-initial `;`.
            (
                "fn main() {\n    let x = 1;\n    {\n    }\n}",
                vec![(BraceOnNewLine, 3, 5)],
            ),
            (
                "fn main() {\n    let x = 1\n    ; { }\n}",
                vec![(BraceOnNewLine, 3, 7)],
            ),
            (
                "fn main() {\n    let x = 1;\n    ;\n    { }\n}",
                vec![(BraceOnNewLine, 4, 5)],
            ),
            (
                "fn main() {\n    let t = a\n    ; - b\n}",
                vec![(LeadingBinaryMinus, 3, 7)],
            ),
            // After a `{`, a `;` does not make the minus leading, as a line break does not.
            ("fn f() -> i64 {\n;\n- 1\n}", vec![]),
            // Inside `[ ]` the `;` separates the parts of an array: it is not skipped.
            ("fn main() { let b = [0;\n{ 1 }] }", vec![]),
            // The skipped `$` hides the minus, with or without the `;`.
            (
                "fn main() {\n    let x = 1 $\n    ; - 2\n}",
                vec![(UnexpectedCharacter, 2, 15)],
            ),
        ] {
            assert_eq!(found(src), want, "{src}");
        }
    }

    #[test]
    fn a_semicolon_is_transparent_to_the_moved_brace() {
        use TokenKind::{Eof, LBrace, Newline, RBrace};
        // The `{` pops the `Newline` that stands before the line's `;`s, or after the previous
        // line's, as it would pop the one that stands before it without them. The `;`s are
        // then redundant, and dropped.
        for src in ["x\n; {}", "x;\n{}", "x;\n;\n{}", "x;;\n{}"] {
            assert_eq!(
                recovered_kinds(src),
                [ident("x"), LBrace, RBrace, Newline, Eof],
                "{src}"
            );
            assert_eq!(found(src).len(), 1, "{src}");
        }
        // No `Newline` after the `{`, with the `;` or without it.
        assert_eq!(all_kinds("{;\n}"), [LBrace, RBrace, Newline, Eof]);
    }

    #[test]
    fn a_cut_interpolation_leaves_no_semicolon_count() {
        use TokenKind::{Eof, Eq, Fn, LBrace, LParen, Let, Newline, RBrace, RParen, Str};
        // The line break cuts the literal. A `;` in the interpolation, or in a block in it, is
        // not among the tokens that remain, so the count of them is not left stale: the `Newline`
        // after the literal still comes, and nothing underflows.
        for src in [
            "fn main() {\n    let s = \"{a;\n}",
            "fn main() {\n    let s = \"{;;\n}",
            "fn main() {\n    let s = \"{ { a;\n}",
            "fn main() {\n    let s = \"{ { a;;\n}",
        ] {
            assert_eq!(
                recovered_kinds(src),
                [
                    Fn,
                    ident("main"),
                    LParen,
                    RParen,
                    LBrace,
                    Let,
                    ident("s"),
                    Eq,
                    Str(String::new()),
                    Newline,
                    RBrace,
                    Newline,
                    Eof
                ],
                "{src}"
            );
            assert_eq!(found(src), [(Code::LiteralNotClosed, 2, 13)], "{src}");
        }
    }

    #[test]
    fn brace_on_new_line_after_a_semicolon_joins_its_statement() {
        // The join replaces the gap by a space, so a gap that holds a `;` makes it unsafe. The
        // statement is the one after the last `;` outside brackets.
        for (src, safe, joined) in [
            (
                "fn main() {\n    let x = 1; if c\n    { }\n}",
                true,
                "fn main() {\n    let x = 1; if c { }\n}",
            ),
            (
                "fn main() {\n    if c\n    ; { }\n}",
                false,
                "fn main() {\n    if c { }\n}",
            ),
            (
                "fn main() {\n    if c;\n    { }\n}",
                false,
                "fn main() {\n    if c { }\n}",
            ),
            ("let x = 1; if f(1)\n{ }", true, "let x = 1; if f(1) { }"),
            // Neither does a `;` inside `[ ]` or in an interpolation.
            (
                "fn main() {\n    if xs == [0; 3]\n    { }\n}",
                true,
                "fn main() {\n    if xs == [0; 3] { }\n}",
            ),
            (
                "fn main() {\n    if \"{a; b}\" == c\n    { }\n}",
                true,
                "fn main() {\n    if \"{a; b}\" == c { }\n}",
            ),
        ] {
            let d = only(src);
            assert_eq!(d.code(), Code::BraceOnNewLine, "{src}");
            assert_eq!(fix_edits(&d).map(|(safe, _)| safe), Some(safe), "{src}");
            assert_eq!(apply_fix(src, &d), joined, "{src}");
        }
    }

    #[test]
    fn dump_format() {
        let src = "fn main() { loop {} }\n";
        assert_eq!(
            dump(src, &tokens(src)),
            "1:1 Fn fn\n1:4 Ident main\n1:8 LParen (\n1:9 RParen )\n1:11 LBrace {\n1:13 Loop loop\n1:18 LBrace {\n1:19 RBrace }\n1:21 RBrace }\n1:22 Newline\n1:22 Eof\n"
        );
    }

    #[test]
    fn reports_every_bad_character() {
        let src = "fn main() {\n    let a = 1 $ 2\n    let b = 3 # 4\n}";
        let lexed = lex(src).expect("the lexer recovers");
        let found: Vec<_> = lexed.diagnostics.iter().map(|d| at(src, d)).collect();
        assert_eq!(
            found,
            [
                (Code::UnexpectedCharacter, 2, 15),
                (Code::UnexpectedCharacter, 3, 15)
            ]
        );
        let kinds: Vec<_> = lexed.tokens.into_iter().map(|t| t.kind).collect();
        assert!(kinds.contains(&int(2)), "{kinds:?}");
        assert!(kinds.contains(&int(4)), "{kinds:?}");
    }

    #[test]
    fn bracket_error_yields_to_a_skip() {
        let src = "fn main() {\n    $\n    print((1)\n}";
        assert_eq!(at(src, &fatal(src)), (Code::UnexpectedCharacter, 2, 5));
    }

    #[test]
    fn unclosed_string_that_unbalances_should_recover() {
        let src = "fn main() { print(\"abc) }";
        assert_eq!(at(src, &only(src)), (Code::LiteralNotClosed, 1, 19));
    }

    /// The literal ate the `)`, so the `}` pops the `(` too.
    #[test]
    fn closer_after_unclosed_string_should_pop_to_its_opener() {
        let src = "{ f(\"a\n}\n";
        assert_eq!(at(src, &only(src)), (Code::LiteralNotClosed, 1, 5));
        assert!(lex(src).is_ok_and(|l| l.tokens.iter().any(|t| t.kind == TokenKind::RBrace)));
    }

    /// The `(` opens after the literal, which cannot have eaten its `)`: the `}` is a real
    /// bracket error, so it does not pop the `(`.
    #[test]
    fn closer_after_unclosed_string_should_not_pop_an_opener_after_the_string() {
        let src = "{ \"a\nf(\n}\n";
        assert_eq!(at(src, &fatal(src)), (Code::LiteralNotClosed, 1, 3));
    }

    #[test]
    fn stray_closer_after_unclosed_string_should_be_dropped() {
        let src = "\"a\n)\n";
        assert_eq!(at(src, &only(src)), (Code::LiteralNotClosed, 1, 1));
        assert!(lex(src).is_ok_and(|l| l.tokens.iter().all(|t| t.kind != TokenKind::RParen)));
    }

    /// The `)` is text of the interpolation to pop to, not the `(` below its `{`.
    #[test]
    fn closer_in_interpolation_after_unclosed_string_should_not_pop_below_the_frame() {
        let src = "f(\"a\n\"{x)}\"\n";
        assert_eq!(at(src, &only(src)), (Code::LiteralNotClosed, 1, 3));
        assert!(lex(src).is_ok_and(|l| l.tokens.iter().all(|t| t.kind != TokenKind::RParen)));
    }

    /// The closers of the popped brackets, innermost first, have no width and sit at the closer.
    #[test]
    fn popped_brackets_should_get_closers_of_no_width_in_order() {
        use TokenKind::{RBrace, RBracket, RParen};
        for (src, closer, added) in [
            ("f({[ \"a\n)\n", RParen, [RBracket, RBrace]),
            ("[({ \"a\n]\n", RBracket, [RBrace, RParen]),
            ("{[( \"a\n}\n", RBrace, [RParen, RBracket]),
        ] {
            let at = src.find(['\n']).expect("a line end") + 1;
            let tokens = lex(src).expect("the lexer recovers").tokens;
            let after: Vec<_> = tokens.iter().filter(|t| t.span.start() >= at).collect();
            let [first, second, last, ..] = after.as_slice() else {
                panic!("expected the closers and a closer in {src:?}");
            };
            assert_eq!(
                (&first.kind, &second.kind, &last.kind),
                (&added[0], &added[1], &closer)
            );
            assert!(first.span.range().is_empty() && second.span.range().is_empty());
            assert_eq!((first.span.start(), second.span.start()), (at, at));
        }
    }

    /// The `}` finds the interpolation's own `{` only at the frame's brace, which it must not pop.
    #[test]
    fn closer_of_the_interpolation_after_unclosed_string_should_be_dropped() {
        let src = "\"a\n\"{g(x}\"\n";
        let ds = recovered(src);
        let codes: Vec<_> = ds.iter().map(|d| at(src, d)).collect();
        assert_eq!(
            codes,
            [
                (Code::LiteralNotClosed, 1, 1),
                (Code::LiteralNotClosed, 2, 7)
            ]
        );
    }

    #[test]
    fn plain_bracket_error_should_stay_fatal_in_a_longer_source() {
        assert_eq!(fatal("f(]").code(), Code::UnexpectedClosingBracket);
    }

    #[test]
    fn bracket_error_alone_is_reported() {
        let d = fatal("(}");
        assert_eq!(d.code(), Code::UnexpectedClosingBracket);
        assert_eq!(d.span(), Span::new(1, 2));
    }

    /// A bracket in a string is text.
    #[test]
    fn string_holds_brackets() {
        use TokenKind::{Eof, LParen, Newline, RParen};
        assert_eq!(
            all_kinds("print(\"a)\")\n"),
            [ident("print"), LParen, str("a)"), RParen, Newline, Eof]
        );
    }

    /// A string that is not closed takes the rest of its line, and lexing goes on at the next.
    #[test]
    fn string_without_a_close_ends_at_the_line() {
        use TokenKind::{Eof, Eq, Let, Newline};
        let src = "let n = \"abc\nlet m = 1\n";
        let lexed = lex(src).expect("the lexer recovers");
        assert_eq!(lexed.diagnostics.len(), 1, "{:?}", lexed.diagnostics);
        let kinds: Vec<_> = lexed.tokens.iter().map(|t| t.kind.clone()).collect();
        assert_eq!(
            kinds,
            [
                Let,
                ident("n"),
                Eq,
                str("abc"),
                Newline,
                Let,
                ident("m"),
                Eq,
                int(1),
                Newline,
                Eof
            ]
        );
        assert_eq!(line_col(src, lexed.tokens[5].span.start()), (2, 1));
    }

    #[test]
    fn quote_at_end_of_file() {
        let src = "fn main() {}\n\"";
        let d = only(src);
        assert_eq!(at(src, &d), (Code::LiteralNotClosed, 2, 1));
        assert_eq!(d.span(), Span::new(13, 14));
    }

    #[test]
    fn bad_char_at_end_of_file() {
        assert_eq!(err("fn main() {}\n$"), (Code::UnexpectedCharacter, 2, 1));
    }

    #[test]
    fn brace_on_its_own_line_lexes_as_one_line() {
        let lexed = lex("fn main()\n{\n}").expect("the lexer recovers");
        assert_eq!(lexed.diagnostics.len(), 1, "{:?}", lexed.diagnostics);
        let kinds: Vec<_> = lexed.tokens.into_iter().map(|t| t.kind).collect();
        assert_eq!(kinds, all_kinds("fn main() {\n}"));
        // After `else` no line break was pushed, so none is dropped.
        let lexed = lex("fn main() { print(if c { 1 } else\n{ 2 }) }").expect("the lexer recovers");
        let kinds: Vec<_> = lexed.tokens.into_iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            all_kinds("fn main() { print(if c { 1 } else { 2 }) }")
        );
    }

    /// Diagnostics come in source order, from literals, interpolations, escapes and stray text
    /// alike: the parser finds an item's diagnostics by binary search.
    #[test]
    fn diagnostics_come_in_source_order() {
        for src in [
            "fn main() {\n    print(\"{1:zz} {0X1}\") // c $\n    let c = 'ab' $ \"q\n}\n",
            "fn main() {\n    let s = \"\\q {0X1:>} \\z\n    print(0X2)\n}\n",
            "fn main() {\n    print(\"a{\"b{0X1}\"}\" $ 0B1)\n}\n",
        ] {
            let lexed = lex(src).expect("the lexer recovers");
            let starts: Vec<usize> = lexed.diagnostics.iter().map(|d| d.span().start()).collect();
            assert!(starts.len() > 2 && starts.is_sorted(), "{src}: {starts:?}");
        }
    }

    #[test]
    fn bad_literal_lexes_as_zero() {
        let lexed = lex("let a = 1_\nlet b = 2").expect("the lexer recovers");
        assert_eq!(lexed.diagnostics.len(), 1, "{:?}", lexed.diagnostics);
        let ints: Vec<_> = lexed
            .tokens
            .into_iter()
            .map(|t| t.kind)
            .filter(|k| matches!(k, TokenKind::Int(_)))
            .collect();
        assert_eq!(ints, [int(0), int(2)]);
    }

    /// The `(code, line, col)` of every diagnostic of `src`, which the lexer recovers from.
    fn found(src: &str) -> Vec<(Code, usize, usize)> {
        let lexed = lex(src).expect("the lexer recovers");
        lexed.diagnostics.iter().map(|d| at(src, d)).collect()
    }

    #[test]
    fn a_skip_damages_its_line() {
        use Code::{
            BraceOnNewLine, BracketOnNewLine, CommentInInterpolation, ElseOnNewLine,
            LeadingBinaryMinus, LiteralNotClosed, UnexpectedCharacter,
        };
        for (src, want) in [
            // Text skipped on the previous line: the line-start error may be its consequence.
            (
                "fn main() {\n    let a = 1 &\n        - 2\n}",
                vec![(UnexpectedCharacter, 2, 15)],
            ),
            (
                "fn main() {\n    f(a $\n        [0])\n}",
                vec![(UnexpectedCharacter, 2, 9)],
            ),
            (
                "fn main() {\n    if true &\n    {\n    }\n}",
                vec![(UnexpectedCharacter, 2, 13)],
            ),
            // A string that is not closed loses the rest of its line too.
            (
                "fn main() {\n    let x = 1 \"+\n        - 2\n}",
                vec![(LiteralNotClosed, 2, 15)],
            ),
            (
                "fn main() {\n    let x = \"{a\n        - 2\n}",
                vec![(LiteralNotClosed, 2, 13)],
            ),
            (
                "fn main() {\n    let x = \"{a // c\n        - 2\n}",
                vec![(CommentInInterpolation, 2, 17)],
            ),
            (
                "fn main() {\n    if don$e\n    {\n    }\n}",
                vec![(UnexpectedCharacter, 2, 11)],
            ),
            ("fn main()$\n{\n}", vec![(UnexpectedCharacter, 1, 10)]),
            (
                "fn main() { print(if c { 1 }$\nelse { 2 }) }",
                vec![(UnexpectedCharacter, 1, 29)],
            ),
            // A comment or blank line between does not repair the damaged line.
            (
                "fn main() {\n    let a = 1 $\n    // c\n\n    - 2\n}",
                vec![(UnexpectedCharacter, 2, 15)],
            ),
            // The skip starts the previous line.
            ("let t = a\n$b\n- c", vec![(UnexpectedCharacter, 2, 1)]),
            // Text skipped earlier on the current line.
            (
                "fn main() {\n    print(1)\n    $ { print(2) }\n}",
                vec![(UnexpectedCharacter, 3, 5)],
            ),
            ("let t = a\n$ - b", vec![(UnexpectedCharacter, 2, 1)]),
            // Independent: the skip is after the token, or before the previous line.
            (
                "let t = a\n- $ b",
                vec![(LeadingBinaryMinus, 2, 1), (UnexpectedCharacter, 2, 3)],
            ),
            (
                "fn main() {\n    let s = 1 $ 2\n    let t = s\n        - 1\n}",
                vec![(UnexpectedCharacter, 2, 15), (LeadingBinaryMinus, 4, 9)],
            ),
            (
                "fn main() {\n    let a = 1$\n    print(a)\n    {\n    }\n}",
                vec![(UnexpectedCharacter, 2, 14), (BraceOnNewLine, 4, 5)],
            ),
            (
                "fn main() {\n    f(a $ b,\n        c\n        [0])\n}",
                vec![(UnexpectedCharacter, 2, 9), (BracketOnNewLine, 4, 9)],
            ),
            (
                "fn main() {\n    print(if c $ {\n        1 }\n    else { 2 })\n}",
                vec![(UnexpectedCharacter, 2, 16), (ElseOnNewLine, 4, 5)],
            ),
        ] {
            assert_eq!(found(src), want, "{src}");
        }
    }

    #[test]
    fn each_brace_fix_holds_alone() {
        // Each safe join, applied alone, leaves the other diagnostics where they were, and the
        // result parses as lexed, with no other join applied.
        for (src, left) in [
            (
                "fn main()\n{\n    if true\n    {\n    }\n}",
                vec![(Code::BraceOnNewLine, 3, 5), (Code::BraceOnNewLine, 2, 1)],
            ),
            // The `$` line is not the one before the `{`.
            (
                "fn f() {\n    let a = 1$\n}\nfn main()\n{\n}",
                vec![(Code::UnexpectedCharacter, 2, 14)],
            ),
        ] {
            let safe: Vec<_> = recovered(src)
                .into_iter()
                .filter(|d| fix_edits(d).is_some_and(|(safe, _)| safe))
                .collect();
            assert_eq!(safe.len(), left.len(), "{src}: {safe:?}");
            for (d, want) in safe.iter().zip(left) {
                let fixed = apply_fix(src, d);
                assert_eq!(found(&fixed), [want], "{fixed}");
                // The lexer's diagnostic stays the only one, and the tokens parse as lexed.
                let lexed = lex(&fixed).expect("the lexer recovers");
                let tokens = lexed.tokens.clone();
                let parsed = crate::parser::parse(lexed);
                let reported: Vec<_> = parsed.diagnostics.iter().map(|d| at(&fixed, d)).collect();
                assert_eq!(reported, [want], "{fixed}");
                let parsed = crate::parser::parse(Lexed {
                    source: &fixed,
                    tokens,
                    diagnostics: Vec::new(),
                    warnings: Vec::new(),
                    comments: Vec::new(),
                    breaks: Vec::new(),
                });
                assert!(
                    parsed.diagnostics.is_empty() && parsed.checkable,
                    "{fixed}: {:?}",
                    parsed.diagnostics
                );
            }
        }
    }

    /// A `>` at a line's end, and a `{` that starts the line after it, read their statement
    /// without scanning back to its start, so a comparison continued over many lines is read
    /// once: with plain operands, block operands (each `{` asks whether a header is open), and
    /// block operands with comparisons of their own, bare and in parentheses (an inner block's
    /// `>` must not cost the outer statement its place).
    #[test]
    fn continued_comparisons_should_be_judged_linearly() {
        for operand in ["a", "{ a }", "{ a >\n a }", "({ a >\n a })"] {
            let judged = |n: usize| {
                let chain = vec![operand; n].join(" >\n");
                JUDGED.set(0);
                lex_clean(&format!("fn f() {{\n    let b: bool = {chain}\n}}"))
                    .expect("the source should lex");
                JUDGED.get()
            };
            crate::mir::tests::assert_near_linear(judged(1000), judged(2000), judged(3000));
        }
    }

    /// A header's list that is not closed, or that holds a token other than a name or `,`,
    /// ends at its line break as before, and so does each line after it. A name on the next
    /// line goes on with the list, but the line break after that name ends the statement.
    #[test]
    fn a_malformed_header_list_should_keep_the_line_breaks_after_it() {
        use TokenKind::{Comma, Eof, Eq, Fn, LBrace, LParen, Let, Lt, Newline, RBrace, RParen};
        for (src, expected) in [
            (
                "fn f<T\n{\n    let a = 1\n    let b = 2\n}\nfn g() {}",
                vec![
                    Fn,
                    ident("f"),
                    Lt,
                    ident("T"),
                    LBrace,
                    Let,
                    ident("a"),
                    Eq,
                    int(1),
                    Newline,
                    Let,
                    ident("b"),
                    Eq,
                    int(2),
                    Newline,
                    RBrace,
                    Newline,
                    Fn,
                    ident("g"),
                    LParen,
                    RParen,
                    LBrace,
                    RBrace,
                    Newline,
                    Eof,
                ],
            ),
            (
                "fn f<T, 1\n    let a = 1\n    let b = 2",
                vec![
                    Fn,
                    ident("f"),
                    Lt,
                    ident("T"),
                    Comma,
                    int(1),
                    Newline,
                    Let,
                    ident("a"),
                    Eq,
                    int(1),
                    Newline,
                    Let,
                    ident("b"),
                    Eq,
                    int(2),
                    Newline,
                    Eof,
                ],
            ),
            (
                "fn f<T\n    a = 1\n    b = 2",
                vec![
                    Fn,
                    ident("f"),
                    Lt,
                    ident("T"),
                    ident("a"),
                    Eq,
                    int(1),
                    Newline,
                    ident("b"),
                    Eq,
                    int(2),
                    Newline,
                    Eof,
                ],
            ),
        ] {
            let lexed = lex(src).expect("the lexer should recover");
            let kinds: Vec<_> = lexed.tokens.into_iter().map(|t| t.kind).collect();
            assert_eq!(kinds, expected, "{src}");
        }
    }

    /// Only a name goes on with a list: a number is no name, and after a `_` that stands where
    /// the header's name goes, the `<` opens no list.
    #[test]
    fn a_header_list_should_go_on_only_with_names() {
        for src in ["fn f<T\n    1>() {}", "fn _<\n    T\n>() {}"] {
            let kinds = all_kinds(src);
            let t = kinds.iter().position(|k| *k == ident("T"));
            let after = t.and_then(|t| kinds.get(t + 1));
            assert_eq!(after, Some(&TokenKind::Newline), "{src}");
        }
    }

    /// A list continued at the start of each line, so that each line break asks about it.
    #[test]
    fn a_long_header_list_over_many_lines_should_be_judged_once_per_token() {
        let params: Vec<String> = (0..1000).map(|i| format!("T{i}")).collect();
        let src = format!("fn f<\n    {}\n>(x: T0) {{}}", params.join("\n    , "));
        JUDGED.set(0);
        let tokens = tokens(&src);
        assert!(
            tokens[..tokens.len() - 2]
                .iter()
                .all(|t| t.kind != TokenKind::Newline),
            "the header should hold no `Newline`"
        );
        assert!(JUDGED.get() <= tokens.len(), "{} judged", JUDGED.get());
    }

    /// Blank lines and comments between two parameters, each of which a line break starts.
    #[test]
    fn a_long_gap_in_a_header_list_should_be_looked_through_once() {
        let gap = format!("\n{}\n    ", "\n// c".repeat(1000));
        let src = format!("fn f<\n    T{gap}, U>(a: T, b: U) {{}}");
        LOOKED.set(0);
        let tokens = tokens(&src);
        assert!(
            tokens[..tokens.len() - 2]
                .iter()
                .all(|t| t.kind != TokenKind::Newline),
            "the header should hold no `Newline`"
        );
        assert!(
            LOOKED.get() <= gap.len(),
            "{} bytes looked at",
            LOOKED.get()
        );
    }
}
