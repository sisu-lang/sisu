//! The parser: tokens to the syntax tree.

use std::ops::Range;

use crate::ast::{
    Align, Arg, Arm, BinaryOp, Block, Bound, Broken, BrokenKind, Class, CompareOp, Const, Enum,
    Expr, ExprKind, FieldDecl, ForSource, FormatSpec, Function, Ident, Item, Literal, Member,
    Param, Part, Pattern, PatternKind, PayloadField, Program, Stmt, StmtKind, TypeExpr, TypeKind,
    UnaryOp, Variant,
};
use crate::diagnostic::{
    self, BuiltIn, ClashForm, Diagnostic, DiagnosticKind, ForeignPattern, ForeignRange, Join,
    LetField, MatchIdiomForm, Owner, PowerFix,
};
use crate::lexer::{Lexed, MIN_MAGNITUDE, Token, TokenKind, literal_too_large};
use crate::source::Span;
use crate::symbol::Symbol;

/// What the parser makes of a lexed source.
#[derive(Debug)]
pub(crate) struct Parsed {
    /// The intact items, and the names of the broken ones.
    pub(crate) program: Program,
    /// One error per broken item, each lexer diagnostic outside every item, and every lexer
    /// warning, by position.
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// Whether every item is named, so that no use of a lost item can read as "cannot find".
    pub(crate) checkable: bool,
}

/// Parses `lexed`, whose tokens end with `Eof` and whose brackets balance. An item with a
/// syntax error, its own or the lexer's, is broken: it reports its earliest error, and the
/// parser resumes after it. A bad format spec alone does not break its item: the parser reports
/// it and reads its part without a spec. A lexer warning breaks nothing.
pub(crate) fn parse(lexed: Lexed<'_>) -> Parsed {
    let Lexed {
        source,
        tokens,
        diagnostics,
        warnings,
        comments,
        breaks,
    } = lexed;
    debug_assert!(
        diagnostics.is_sorted_by_key(|d| d.span().start()),
        "`lexer_in` searches the lexer's diagnostics by where they start"
    );
    Parser {
        source,
        tokens: &tokens,
        pos: 0,
        depth: 0,
        lexer: &diagnostics,
        comments: &comments,
        breaks: &breaks,
        specs: Vec::new(),
        warnings,
        labels: Vec::new(),
    }
    .program()
}

/// The program of `source`, which must lex and parse without a diagnostic.
#[cfg(test)]
pub(crate) fn parse_ok(source: &str) -> Program {
    let parsed = parse(crate::lexer::lex(source).expect("the source should lex"));
    assert!(
        parsed.diagnostics.is_empty() && parsed.checkable,
        "{source}: {:?}",
        parsed.diagnostics
    );
    parsed.program
}

/// The earlier of `a` and `b`; `a` on a tie.
fn earlier(a: Diagnostic, b: Diagnostic) -> Diagnostic {
    if b.span().start() < a.span().start() {
        b
    } else {
        a
    }
}

/// Whether one of `ranges` holds `at`, given that neither their starts nor their ends go back
/// (`rising`): the last to start at or before `at` holds it if any does.
fn holds(ranges: &[Range<usize>], at: usize) -> bool {
    let i = ranges.partition_point(|r| r.start <= at);
    i > 0 && ranges[i - 1].contains(&at)
}

/// Whether neither the starts nor the ends of `ranges` go back, as `holds` needs.
fn rising(ranges: &[Range<usize>]) -> bool {
    ranges.is_sorted_by(|a, b| a.start <= b.start && a.end <= b.end)
}

/// The format spec `text`, written at `at` from its `:`: `[<^>][width][.precision]`, with a
/// width from 1 to 1000 and a precision from 0 to 30.
fn format_spec(text: &str, at: Span) -> Result<FormatSpec, Diagnostic> {
    let align = match text.as_bytes().first() {
        Some(b'<') => Some(Align::Left),
        Some(b'>') => Some(Align::Right),
        Some(b'^') => Some(Align::Center),
        _ => None,
    };
    // An alignment is one ASCII byte.
    let rest = &text[usize::from(align.is_some())..];
    let digits = |s: &str| s.len() - s.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let (width, rest) = rest.split_at(digits(rest));
    let (precision, rest) = match rest.strip_prefix('.') {
        Some(after) => {
            let (precision, rest) = after.split_at(digits(after));
            (Some(precision), rest)
        }
        None => (None, rest),
    };
    if !rest.is_empty() || width.starts_with('0') {
        return Err(DiagnosticKind::UnsupportedFormatSpec { at }.into());
    }
    if precision.is_some_and(str::is_empty) || (width.is_empty() && precision.is_none()) {
        return Err(DiagnosticKind::MalformedFormatSpec { at }.into());
    }
    let limit = |width| Diagnostic::from(DiagnosticKind::FormatSpecLimit { at, width });
    let width = match width {
        "" => None,
        w => Some(
            w.parse::<u16>()
                .ok()
                .filter(|&w| w <= 1000)
                .ok_or_else(|| limit(true))?,
        ),
    };
    let precision = match precision {
        None => None,
        Some(p) => Some(
            p.parse::<u8>()
                .ok()
                .filter(|&p| p <= 30)
                .ok_or_else(|| limit(false))?,
        ),
    };
    Ok(FormatSpec {
        align,
        width,
        precision,
        span: at,
    })
}

/// How deep the syntax tree may nest. Every later stage walks the tree recursively, and so does
/// dropping it, so the parser is the one place that bounds the stack they need.
const MAX_DEPTH: usize = 256;

/// An infix operator other than the comparisons of level 4.
enum Infix {
    Binary(BinaryOp),
    Is,
    Coalesce,
}

/// The level and operator of an infix operator token.
fn infix_op(kind: &TokenKind) -> Option<(u8, Infix)> {
    Some(match kind {
        TokenKind::OrOr => (1, Infix::Binary(BinaryOp::Or)),
        TokenKind::AndAnd => (2, Infix::Binary(BinaryOp::And)),
        TokenKind::EqEq => (3, Infix::Binary(BinaryOp::Eq)),
        TokenKind::NotEq => (3, Infix::Binary(BinaryOp::Ne)),
        TokenKind::Is => (3, Infix::Is),
        TokenKind::QuestionQuestion => (5, Infix::Coalesce),
        TokenKind::Plus => (6, Infix::Binary(BinaryOp::Add)),
        TokenKind::Minus => (6, Infix::Binary(BinaryOp::Sub)),
        TokenKind::Star => (7, Infix::Binary(BinaryOp::Mul)),
        TokenKind::Slash => (7, Infix::Binary(BinaryOp::Div)),
        TokenKind::Percent => (7, Infix::Binary(BinaryOp::Rem)),
        _ => return None,
    })
}

/// The level of `**`, which binds as `*` does.
const POWER_LEVEL: u8 = 7;

/// Whether `e`, whose tokens are `tokens`, is a plain operand of a `**` that `.pow` can replace:
/// a name, a float literal, a call, a field, a method call, an index, or anything in
/// parentheses. The right side may be any literal too.
fn power_operand(e: &Expr, tokens: &[Token], right: bool) -> bool {
    match e.kind {
        ExprKind::Name(_)
        | ExprKind::Float(_)
        | ExprKind::Call { .. }
        | ExprKind::Field { .. }
        | ExprKind::MethodCall { .. }
        | ExprKind::Index { .. } => true,
        ExprKind::Int { .. } | ExprKind::Bool(_) | ExprKind::Char(_) | ExprKind::Str(_) => right,
        _ => parenthesised(tokens),
    }
}

/// Whether one pair of parentheses holds the whole of `tokens`: it starts with a `(` whose `)`
/// is the last token. `(a) * (b)` does not.
fn parenthesised(tokens: &[Token]) -> bool {
    let [first, inner @ .., last] = tokens else {
        return false;
    };
    if first.kind != TokenKind::LParen || last.kind != TokenKind::RParen {
        return false;
    }
    let mut open = 1usize;
    for token in inner {
        match token.kind {
            TokenKind::LParen => open += 1,
            TokenKind::RParen => {
                open -= 1;
                if open == 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

/// Whether `kind` begins a postfix link (`.`, `?.` or `[`); `postfix` and `at_min_literal`
/// share it.
fn starts_postfix(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Dot | TokenKind::QuestionDot | TokenKind::LBracket
    )
}

fn compare_op(kind: &TokenKind) -> Option<CompareOp> {
    Some(match kind {
        TokenKind::Lt => CompareOp::Lt,
        TokenKind::Le => CompareOp::Le,
        TokenKind::Gt => CompareOp::Gt,
        TokenKind::Ge => CompareOp::Ge,
        _ => return None,
    })
}

/// One line of an enum body.
enum EnumMember {
    Variant(Variant),
    Method(Function),
}

/// A range operator as written.
#[derive(Clone, Copy)]
enum RangeOp {
    /// `..`.
    Exclusive,
    /// `..=`.
    Inclusive,
    /// Another language's `..<` (`lt` is the `<`) or `...`, with the span of the operator.
    Foreign { at: Span, lt: Option<Span> },
}

struct Parser<'a> {
    /// The text the token spans index.
    source: &'a str,
    tokens: &'a [Token],
    pos: usize,
    /// While a node's children are parsed: the levels above them. When a parse function returns:
    /// the height of the deepest path through the tree it built, counted from the level it was
    /// entered at, in the same units. A node that wraps what it already parsed, as an operator
    /// or a `.` does in a chain, adds a level to that height, so no way of building a tree gets
    /// past `MAX_DEPTH`. An error ends the parse.
    depth: usize,
    /// The lexer's diagnostics, in source order.
    lexer: &'a [Diagnostic],
    /// Where each comment starts.
    comments: &'a [usize],
    /// Where each line break is.
    breaks: &'a [usize],
    /// The error of each bad format spec so far that no broken item took.
    specs: Vec<Diagnostic>,
    /// The lexer's warnings, which `program` reports whether or not their item is broken.
    warnings: Vec<Diagnostic>,
    /// The loop labels open around the next token, innermost last: from each one's `name:` to the
    /// end of its loop's body.
    labels: Vec<Ident>,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> &'a Token {
        // `Eof` is last and never consumed, so `pos` stays in range.
        &self.tokens[self.pos]
    }

    fn bump(&mut self) -> &'a Token {
        let token = self.peek();
        if token.kind != TokenKind::Eof {
            self.pos += 1;
        }
        token
    }

    fn at(&self, kind: &TokenKind) -> bool {
        self.peek().kind == *kind
    }

    /// Consumes the next token if it is `kind`.
    fn eat(&mut self, kind: &TokenKind) -> Option<&'a Token> {
        self.at(kind).then(|| self.bump())
    }

    /// Sets `self.depth`, or reports that the tree is too deep at the next token.
    fn reach(&mut self, depth: usize) -> Result<(), Diagnostic> {
        self.depth = depth;
        if depth > MAX_DEPTH {
            return Err(DiagnosticKind::NestingTooDeep {
                at: self.peek().span,
                limit: MAX_DEPTH,
            }
            .into());
        }
        Ok(())
    }

    /// Enters one more level of nesting for the children that follow.
    fn descend(&mut self) -> Result<(), Diagnostic> {
        self.reach(self.depth + 1)
    }

    /// Parses one child of a node whose children start at depth `base`, and raises `reach`, the
    /// height of the node so far, to the child's. Every parse function follows the same pattern:
    /// reset `self.depth` to the base before each child, and keep the maximum in `reach`.
    fn child<T>(
        &mut self,
        base: usize,
        reach: &mut usize,
        parse: impl FnOnce(&mut Self) -> Result<T, Diagnostic>,
    ) -> Result<T, Diagnostic> {
        self.depth = base;
        let node = parse(self)?;
        *reach = (*reach).max(self.depth);
        Ok(node)
    }

    fn skip_newlines(&mut self) {
        while self.eat(&TokenKind::Newline).is_some() {}
    }

    /// What must follow a statement or a class member: a line break, a `;` that stands in for
    /// one, or the closing `}`. The one place that says "expected end of line or `;`".
    fn end_of_line(&self) -> Diagnostic {
        self.expected("end of line or `;`")
    }

    /// "expected <what>, found <the next token>", pointing at the next token.
    /// A stray `var` or reserved word always gets its own diagnostic, wherever it appears.
    fn expected(&self, what: &str) -> Diagnostic {
        self.expected_at(self.peek(), what)
    }

    /// "expected <what>, found <found>", pointing at the token `found`, which need not be next.
    fn expected_at(&self, found: &Token, what: &str) -> Diagnostic {
        match found.kind {
            TokenKind::Var => DiagnosticKind::Var {
                at: found.span,
                field: false,
            },
            TokenKind::Reserved(word) => DiagnosticKind::ReservedWord {
                word,
                at: found.span,
            },
            TokenKind::Int(_) => DiagnosticKind::Expected {
                what: what.to_string(),
                found: format!("`{}`", &self.source[found.span.range()]),
                at: found.span,
            },
            // A `Newline` over a `;` that separates is named as written.
            TokenKind::Newline if !found.span.range().is_empty() => DiagnosticKind::Expected {
                what: what.to_string(),
                found: "`;`".to_string(),
                at: found.span,
            },
            _ => DiagnosticKind::Expected {
                what: what.to_string(),
                found: found.kind.describe(),
                at: found.span,
            },
        }
        .into()
    }

    fn expect(&mut self, kind: &TokenKind) -> Result<&'a Token, Diagnostic> {
        self.eat(kind)
            .ok_or_else(|| self.expected(&kind.describe()))
    }

    fn ident(&mut self, what: &str) -> Result<Ident, Diagnostic> {
        match self.peek().kind {
            TokenKind::Ident(name) => Ok(Ident {
                name,
                span: self.bump().span,
            }),
            _ => Err(self.expected(what)),
        }
    }

    /// An `ident` that names a function, class, constant, field or parameter, where `_` is an
    /// error.
    fn name(&mut self, what: &str) -> Result<Ident, Diagnostic> {
        if self.at(&TokenKind::Underscore) {
            return Err(DiagnosticKind::UnderscoreName {
                at: self.peek().span,
            }
            .into());
        }
        self.ident(what)
    }

    /// The name a `let` or `for` binds, which may be `_`.
    fn binding(&mut self) -> Result<Ident, Diagnostic> {
        if self.at(&TokenKind::Underscore) {
            return Ok(Ident {
                name: Symbol::intern("_"),
                span: self.bump().span,
            });
        }
        self.ident("a binding name")
    }

    /// The only place that reads a type; later milestones extend it.
    fn parse_type(&mut self) -> Result<TypeExpr, Diagnostic> {
        self.descend()?;
        let start = self.peek().span;
        let (kind, mut span) = if self.eat(&TokenKind::LBracket).is_some() {
            let element = self.parse_type()?;
            let close = self.expect(&TokenKind::RBracket)?;
            (TypeKind::Array(Box::new(element)), start.to(close.span))
        } else {
            let ident = self.ident("a type")?;
            (TypeKind::Name(ident.name), ident.span)
        };
        let optional = self.eat(&TokenKind::Question).is_some();
        if optional {
            span = span.to(self.last_span());
        }
        // `??` is one token, so `Tree??` and `Tree? ?` both stop here.
        if matches!(
            self.peek().kind,
            TokenKind::Question | TokenKind::QuestionQuestion
        ) {
            let base = TypeExpr {
                kind,
                optional: false,
                span,
            };
            return Err(DiagnosticKind::OptionalNests {
                ty: base.to_string(),
                at: start.to(self.peek().span),
            }
            .into());
        }
        Ok(TypeExpr {
            kind,
            optional,
            span,
        })
    }

    /// The program, which keeps the lexer's `breaks`.
    fn program(&mut self) -> Parsed {
        let mut items = Vec::new();
        let mut broken = Vec::new();
        let mut diagnostics = Vec::new();
        let mut checkable = true;
        // Where the items lie; each reports the lexer diagnostics inside it itself.
        let mut covered = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::Eof) {
            self.depth = 0;
            let start = self.pos;
            let tokens = self.tokens;
            let previous = tokens[..start].last();
            let header = self.header(start);
            // An item without a name would be lost, so the checker does not run; it runs on to
            // the next item, so that a foreign function and its body make one error.
            let end = if header.is_some() {
                self.item_end(start)
            } else {
                self.next_line_item(start)
            };
            debug_assert!(
                end > start,
                "an item should hold a token, or the loop would not end"
            );
            // An unnamed item starts where its line does, so text skipped before its first
            // token is its own.
            let line = previous.map_or(0, |t| t.span.end());
            let mut bytes = self.bytes(start, end);
            if header.is_none() {
                bytes.start = line;
            }
            covered.push(bytes);
            let parsed = match self.peek().kind {
                TokenKind::Fn => {
                    self.checked(end, |p| Ok((Item::Function(p.function(None)?), Vec::new())))
                }
                TokenKind::Class => self.with_methods(end, |p, errors, methods| {
                    p.class_body(errors, methods).map(Item::Class)
                }),
                TokenKind::Enum => self.with_methods(end, |p, errors, methods| {
                    p.enum_body(errors, methods).map(Item::Enum)
                }),
                TokenKind::Const => {
                    self.checked(end, |p| Ok((Item::Const(p.constant(end)?), Vec::new())))
                }
                _ => self.checked(end, |p| Err(p.expected("`fn`, `class`, `const` or `enum`"))),
            };
            match parsed {
                Ok((item, errors)) => {
                    debug_assert!(header.is_some(), "an unnamed item should hold an error");
                    items.push(item);
                    diagnostics.extend(errors);
                }
                Err(error) => {
                    if let Some(header) = header {
                        diagnostics.push(error);
                        broken.push(header);
                    } else {
                        let skipped = self.lexer_in(line..error.span().start()).next().cloned();
                        diagnostics.push(skipped.unwrap_or(error));
                        checkable = false;
                    }
                }
            }
            self.skip_newlines();
        }
        diagnostics.append(&mut self.specs);
        diagnostics.append(&mut self.warnings);
        // An unnamed item starts where its line does, inside the item before, so items may overlap.
        debug_assert!(rising(&covered));
        diagnostics.extend(
            self.lexer
                .iter()
                .filter(|d| !holds(&covered, d.span().start()))
                .cloned(),
        );
        diagnostics.sort_by_key(|d| d.span().start());
        Parsed {
            program: Program {
                items,
                broken,
                comments: self.comments.to_vec(),
                quotes: self
                    .tokens
                    .iter()
                    .filter(|t| matches!(t.kind, TokenKind::Str(_) | TokenKind::StrHead(_)))
                    .map(|t| t.span.start())
                    .collect(),
                breaks: self.breaks.to_vec(),
            },
            diagnostics,
            checkable,
        }
    }

    /// The name and kind of the item that starts at `start`, when its header is intact: `fn`,
    /// `class`, `enum` or `const`, then a name, with no lexer diagnostic before the token after the
    /// name. `fn to$tal` lexes as `fn to tal`, so its name is not `to`.
    fn header(&self, start: usize) -> Option<Broken> {
        let kind = match self.tokens[start].kind {
            TokenKind::Fn => BrokenKind::Function,
            TokenKind::Class => BrokenKind::Class,
            TokenKind::Enum => BrokenKind::Enum,
            TokenKind::Const => BrokenKind::Constant,
            _ => return None,
        };
        // Neither a keyword nor a name is `Eof`, so the two tokens after the keyword exist.
        let TokenKind::Ident(name) = &self.tokens[start + 1].kind else {
            return None;
        };
        let header = self.tokens[start].span.start()..self.tokens[start + 2].span.start();
        if self.lexer_in(header).next().is_some() {
            return None;
        }
        Some(Broken {
            name: Ident {
                name: *name,
                span: self.tokens[start + 1].span,
            },
            kind,
        })
    }

    /// The end, exclusive, of the item or member that starts at `start`: past the `}` that
    /// brings the depth back to where it started, or at a line break at that depth, which comes
    /// before any `{`. A closer below that depth (a class's `}`) or `Eof` ends it first. A
    /// constant is one line, as a statement is, so a `}` in its value does not end it.
    fn item_end(&self, start: usize) -> usize {
        let line = self.tokens[start].kind == TokenKind::Const;
        let mut depth = 0usize;
        for (i, token) in self.tokens.iter().enumerate().skip(start) {
            match token.kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                    if depth == 0 {
                        return i;
                    }
                    depth -= 1;
                    if depth == 0 && token.kind == TokenKind::RBrace && !line {
                        return i + 1;
                    }
                }
                TokenKind::Newline if depth == 0 => return i,
                TokenKind::Eof => return i,
                _ => {}
            }
        }
        unreachable!("the tokens should end with `Eof`")
    }

    /// The end, exclusive, of the unnamed item that starts at `start`: the next line whose first
    /// token is `fn`, `class`, `const` or `enum` outside brackets, or `Eof`.
    // shortcut: a line break after a token that cannot end a statement (`:`) leaves no `Newline`,
    // so a `fn` after one is not found; the item only runs longer, and the checker is off anyway.
    fn next_line_item(&self, start: usize) -> usize {
        let mut depth = 0usize;
        for (i, token) in self.tokens.iter().enumerate().skip(start) {
            match token.kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => depth -= 1,
                TokenKind::Fn | TokenKind::Class | TokenKind::Const | TokenKind::Enum
                    if i > start && depth == 0 && self.tokens[i - 1].kind == TokenKind::Newline =>
                {
                    return i;
                }
                TokenKind::Eof => return i,
                _ => {}
            }
        }
        unreachable!("the tokens should end with `Eof`")
    }

    /// The bytes of the item from `start` to `end`, exclusive. An item that ends past its own `}`,
    /// or past the line break before the next item, ends there. Any other, and a constant, which
    /// is one line, runs up to the token that ends it (a line break, a closer or `Eof`), so that
    /// text skipped before it lies in the item.
    fn bytes(&self, start: usize, end: usize) -> Range<usize> {
        let line = self.tokens[start].kind == TokenKind::Const;
        let last = &self.tokens[end - 1];
        let stop = match (&last.kind, &self.tokens[end].kind) {
            (TokenKind::RBrace, _) if !line => last.span.end(),
            // `Eof` sits just after the last code, before any skipped text.
            (_, TokenKind::Eof) => usize::MAX,
            _ => self.tokens[end].span.start(),
        };
        self.tokens[start].span.start()..stop
    }

    /// The lexer diagnostics that start in `bytes`, in order.
    fn lexer_in(&self, bytes: Range<usize>) -> impl Iterator<Item = &Diagnostic> {
        let at = |end: usize| self.lexer.partition_point(|d| d.span().start() < end);
        let start = at(bytes.start);
        self.lexer[start..at(bytes.end).max(start)].iter()
    }

    /// Parses with `parse` the item or member that starts here and runs to `end`. Its error is
    /// the earlier of its first lexer diagnostic and its parse error, which a skip may have
    /// caused. After an error, the parser resumes at `end`.
    fn checked<T>(
        &mut self,
        end: usize,
        parse: impl FnOnce(&mut Self) -> Result<T, Diagnostic>,
    ) -> Result<T, Diagnostic> {
        let bytes = self.bytes(self.pos, end);
        let specs = self.specs.len();
        let parsed = parse(self);
        let lexed = self.lexer_in(bytes).next().cloned();
        if let (Ok(_), None) = (&parsed, &lexed) {
            return parsed;
        }
        let error = self
            .specs
            .drain(specs..)
            .chain(lexed)
            .chain(parsed.err())
            .reduce(earlier)
            .expect("a broken item should hold an error");
        self.pos = end;
        Err(error)
    }

    /// The class or enum that `body` parses, from here to `end`, and the error of each broken
    /// method. A method whose error follows its name is broken alone and leaves the item; any
    /// other error breaks the whole item, whose one error is then the earliest in it.
    fn with_methods(
        &mut self,
        end: usize,
        body: impl FnOnce(
            &mut Self,
            &mut Vec<Diagnostic>,
            &mut Vec<Range<usize>>,
        ) -> Result<Item, Diagnostic>,
    ) -> Result<(Item, Vec<Diagnostic>), Diagnostic> {
        let bytes = self.bytes(self.pos, end);
        let specs = self.specs.len();
        let mut errors = Vec::new();
        let mut methods = Vec::new();
        let parsed = body(self, &mut errors, &mut methods);
        debug_assert!(rising(&methods));
        let outside = self
            .lexer_in(bytes)
            .find(|d| !holds(&methods, d.span().start()))
            .cloned();
        match (parsed, outside) {
            (Ok(item), None) => Ok((item, errors)),
            (parsed, outside) => {
                self.pos = end;
                let error = errors
                    .into_iter()
                    .chain(self.specs.drain(specs..))
                    .chain(outside)
                    .chain(parsed.err())
                    .reduce(earlier)
                    .expect("a broken item should hold an error");
                Err(error)
            }
        }
    }

    /// The class itself, adding each broken method's error to `errors` and its bytes to
    /// `methods`.
    fn class_body(
        &mut self,
        errors: &mut Vec<Diagnostic>,
        methods: &mut Vec<Range<usize>>,
    ) -> Result<Class, Diagnostic> {
        self.expect(&TokenKind::Class)?;
        let name = self.name("a class name")?;
        self.expect(&TokenKind::LBrace)?;
        let mut members = Vec::new();
        let mut broken_methods = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::RBrace) {
            self.depth = 0;
            let start = self.pos;
            let end = self.item_end(start);
            match self.checked(end, Self::member_line) {
                Ok(member) => members.push(member),
                Err(error) => match self.header(start) {
                    Some(Broken {
                        name,
                        kind: BrokenKind::Function,
                    }) => {
                        methods.push(self.bytes(start, end));
                        broken_methods.push(name);
                        errors.push(error);
                    }
                    _ => return Err(error),
                },
            }
            self.skip_newlines();
        }
        self.bump();
        Ok(Class {
            name,
            members,
            broken_methods,
        })
    }

    /// A class member, and the end of its line.
    fn member_line(&mut self) -> Result<Member, Diagnostic> {
        let member = self.member()?;
        if matches!(member, Member::Field(_)) && self.at(&TokenKind::Comma) {
            return Err(self.member_comma(diagnostic::Member::Field));
        }
        if !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Newline) {
            return Err(self.end_of_line());
        }
        Ok(member)
    }

    /// The error for the comma at the next token, which ends a `member`. Its fix deletes a comma
    /// that a line break, the `}` or a `;` follows, since a `;` in its place would be redundant
    /// (W0007), and replaces any other with a `;` that keeps the spaces after it. The edit covers
    /// the comma alone, so text the lexer skipped after it stays.
    fn member_comma(&self, member: diagnostic::Member) -> Diagnostic {
        let comma = self.peek().span;
        // A comma is not `Eof`, so the token after it exists.
        let next = &self.tokens[self.pos + 1];
        let separated = matches!(
            next.kind,
            TokenKind::RBrace | TokenKind::Eof | TokenKind::Semicolon
        ) || self.source[comma.end()..next.span.start()].contains('\n');
        let replacement = if separated { "" } else { ";" }.to_string();
        DiagnosticKind::FieldComma {
            at: comma,
            member,
            replacement,
        }
        .into()
    }

    fn member(&mut self) -> Result<Member, Diagnostic> {
        match self.peek().kind {
            TokenKind::Ident(_) | TokenKind::Underscore | TokenKind::Mut => {
                let mutable = self.eat(&TokenKind::Mut).is_some();
                let name = self.name("a field name")?;
                self.expect(&TokenKind::Colon)?;
                Ok(Member::Field(FieldDecl {
                    mutable,
                    name,
                    ty: self.parse_type()?,
                }))
            }
            TokenKind::Let => Err(DiagnosticKind::FieldLet {
                at: self.peek().span,
                rest: self.field_after_let(),
            }
            .into()),
            TokenKind::Var => Err(DiagnosticKind::Var {
                at: self.peek().span,
                field: true,
            }
            .into()),
            TokenKind::Fn => Ok(Member::Method(self.method(&Owner::Class)?)),
            _ => Err(self.expected("a field or `fn`")),
        }
    }

    /// A method of `owner`, which must take `self`.
    fn method(&mut self, owner: &Owner) -> Result<Function, Diagnostic> {
        let method = self.function(Some(owner))?;
        if method.self_param.is_none() {
            return Err(DiagnosticKind::MethodNeedsSelf {
                at: method.name.span,
            }
            .into());
        }
        Ok(method)
    }

    /// An enum: its variants, one to a line, then its methods; and the errors of the methods it
    /// breaks, as `class_body` does. A variant after a method breaks the enum.
    fn enum_body(
        &mut self,
        errors: &mut Vec<Diagnostic>,
        methods: &mut Vec<Range<usize>>,
    ) -> Result<Enum, Diagnostic> {
        self.expect(&TokenKind::Enum)?;
        // `class` is not `Eof`, so the token after it exists.
        if self.at(&TokenKind::Class)
            && let next = &self.tokens[self.pos + 1]
            && let TokenKind::Ident(_) = next.kind
        {
            let class = self.peek().span;
            return Err(DiagnosticKind::MatchIdiom {
                form: MatchIdiomForm::EnumClass {
                    name: self.source[next.span.range()].to_string(),
                    remove: Span::new(class.start(), next.span.start()),
                },
                at: class,
            }
            .into());
        }
        let name = self.name("an enum name")?;
        let owner = Owner::Enum(name.name.to_string());
        self.expect(&TokenKind::LBrace)?;
        let mut variants = Vec::new();
        let mut fns = Vec::new();
        let mut broken_methods = Vec::new();
        // The `fn` of the first method, broken or not.
        let mut first_method = None;
        self.skip_newlines();
        while !self.at(&TokenKind::RBrace) {
            self.depth = 0;
            let start = self.pos;
            let end = self.item_end(start);
            if first_method.is_none() && self.at(&TokenKind::Fn) {
                first_method = Some(self.peek().span);
            }
            match self.checked(end, |p| p.enum_line(&owner, first_method)) {
                Ok(EnumMember::Variant(variant)) => variants.push(variant),
                Ok(EnumMember::Method(method)) => fns.push(method),
                Err(error) => match self.header(start) {
                    Some(Broken {
                        name,
                        kind: BrokenKind::Function,
                    }) => {
                        methods.push(self.bytes(start, end));
                        broken_methods.push(name);
                        errors.push(error);
                    }
                    _ => return Err(error),
                },
            }
            self.skip_newlines();
        }
        self.bump();
        Ok(Enum {
            name,
            variants,
            methods: fns,
            broken_methods,
        })
    }

    /// A variant or a method of `owner`, and the end of its line. `first_method` is the `fn` of
    /// the first method before this line.
    fn enum_line(
        &mut self,
        owner: &Owner,
        first_method: Option<Span>,
    ) -> Result<EnumMember, Diagnostic> {
        let member = if self.at(&TokenKind::Fn) {
            EnumMember::Method(self.method(owner)?)
        } else {
            let start = self.peek().span;
            let variant = self.variant()?;
            if let Some(first_method) = first_method {
                return Err(DiagnosticKind::VariantAfterMethod {
                    at: start.to(self.last_span()),
                    first_method,
                }
                .into());
            }
            if self.at(&TokenKind::Comma) {
                return Err(self.member_comma(diagnostic::Member::Variant));
            }
            EnumMember::Variant(variant)
        };
        if !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Newline) {
            return Err(self.end_of_line());
        }
        Ok(member)
    }

    /// `Name`, or `Name(payload)`.
    fn variant(&mut self) -> Result<Variant, Diagnostic> {
        if self.at(&TokenKind::NoneKw) {
            return Err(DiagnosticKind::BuiltInName {
                name: "None".to_string(),
                at: self.peek().span,
                kind: BuiltIn::Keyword,
            }
            .into());
        }
        let name = self.ident("a variant or `fn`")?;
        let payload = match self.eat(&TokenKind::LParen) {
            Some(_) => Some(self.payload()?),
            None => None,
        };
        Ok(Variant { name, payload })
    }

    /// The fields of a payload and its `)`; a trailing comma is allowed.
    fn payload(&mut self) -> Result<Vec<PayloadField>, Diagnostic> {
        let mut fields = Vec::new();
        loop {
            self.depth = 0;
            // An `Ident` is not `Eof`, so the token after it exists.
            let labeled = matches!(self.peek().kind, TokenKind::Ident(_))
                && self.tokens[self.pos + 1].kind == TokenKind::Colon;
            let label = if labeled {
                let label = self.ident("a label")?;
                self.bump();
                Some(label)
            } else {
                None
            };
            fields.push(PayloadField {
                label,
                ty: self.parse_type()?,
            });
            if self.eat(&TokenKind::Comma).is_none() || self.at(&TokenKind::RParen) {
                break;
            }
        }
        self.expect(&TokenKind::RParen)?;
        Ok(fields)
    }

    /// What follows the `let` at the next token: a field that ends the line, or anything else,
    /// shown as `name: Type` or, when no field follows, that placeholder.
    fn field_after_let(&self) -> LetField {
        let mut rest = Parser {
            source: self.source,
            tokens: self.tokens,
            pos: self.pos + 1,
            depth: self.depth,
            lexer: self.lexer,
            comments: self.comments,
            breaks: self.breaks,
            specs: Vec::new(),
            warnings: Vec::new(),
            labels: Vec::new(),
        };
        let mutable = rest.eat(&TokenKind::Mut).is_some();
        let mut field = || -> Result<String, Diagnostic> {
            let name = rest.ident("a field name")?;
            rest.expect(&TokenKind::Colon)?;
            Ok(format!("{}: {}", name.name, rest.parse_type()?))
        };
        match field() {
            Ok(_) if matches!(rest.peek().kind, TokenKind::Newline | TokenKind::RBrace) => {
                LetField::Whole {
                    remove: Span::new(
                        self.peek().span.start(),
                        self.tokens[self.pos + 1].span.start(),
                    ),
                }
            }
            Ok(field) => LetField::Other { mutable, field },
            Err(_) => LetField::Other {
                mutable,
                field: "name: Type".to_string(),
            },
        }
    }

    /// `const name: T = value`, which ends the item at `end`.
    fn constant(&mut self, end: usize) -> Result<Const, Diagnostic> {
        self.bump();
        let name = self.name("a constant name")?;
        let ty = match self.eat(&TokenKind::Colon) {
            Some(_) => Some(self.parse_type()?),
            None => None,
        };
        self.depth = 0;
        self.expect(&TokenKind::Eq)?;
        let value = self.expr()?;
        if self.pos != end {
            return Err(self.end_of_line());
        }
        Ok(Const { name, ty, value })
    }

    /// `owner` is the class or enum of a method, where `self` may be the first parameter.
    fn function(&mut self, owner: Option<&Owner>) -> Result<Function, Diagnostic> {
        let fn_token = self.expect(&TokenKind::Fn)?;
        let name = self.name("a function name")?;
        self.expect(&TokenKind::LParen)?;
        let mut self_param = None;
        let mut params = Vec::new();
        if !self.at(&TokenKind::RParen) {
            loop {
                let receiver = owner.filter(|_| self_param.is_none() && params.is_empty());
                if self.at(&TokenKind::Mut) {
                    // `mut` is never `Eof`, so the token after it exists.
                    match (&self.tokens[self.pos + 1].kind, receiver) {
                        (TokenKind::SelfKw, Some(owner)) => {
                            // `self` is not `Eof`, so the token after it exists.
                            let after = &self.tokens[self.pos + 2].kind;
                            return Err(DiagnosticKind::MutSelf {
                                keyword: self.peek().span,
                                receiver: self.tokens[self.pos + 1].span,
                                safe: matches!(after, TokenKind::Comma | TokenKind::RParen),
                                owner: owner.clone(),
                            }
                            .into());
                        }
                        (TokenKind::SelfKw, None) => {
                            self.bump();
                        }
                        (TokenKind::Ident(name), _) => {
                            return Err(DiagnosticKind::MutParameter {
                                name: name.to_string(),
                                at: self.peek().span,
                            }
                            .into());
                        }
                        _ => {}
                    }
                }
                if self.at(&TokenKind::SelfKw) {
                    if receiver.is_none() {
                        return Err(DiagnosticKind::SelfNotFirst {
                            at: self.peek().span,
                        }
                        .into());
                    }
                    self_param = Some(self.bump().span);
                    if self.eat(&TokenKind::Comma).is_none() {
                        break;
                    }
                    continue;
                }
                let name = self.name("a parameter name")?;
                self.expect(&TokenKind::Colon)?;
                let ty = self.parse_type()?;
                self.depth = 0;
                params.push(Param { name, ty });
                if self.eat(&TokenKind::Comma).is_none() {
                    break;
                }
            }
        }
        self.expect(&TokenKind::RParen)?;
        let ret = match self.eat(&TokenKind::Arrow) {
            Some(_) => Some(self.parse_type()?),
            None => None,
        };
        self.depth = 0;
        let body = self.block()?;
        Ok(Function {
            name,
            self_param,
            params,
            ret,
            span: fn_token.span.to(body.span),
            body,
        })
    }

    fn block(&mut self) -> Result<Block, Diagnostic> {
        self.descend()?;
        let base = self.depth;
        let mut reach = base;
        let open = self.expect(&TokenKind::LBrace)?;
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::RBrace) {
            if self.at(&TokenKind::Else) {
                return Err(self.else_on_new_line(stmts.last()));
            }
            let stmt = self.child(base, &mut reach, Self::stmt)?;
            if !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Newline) {
                return Err(self.statement_end(&stmt));
            }
            stmts.push(stmt);
            self.skip_newlines();
        }
        let close = self.bump();
        self.depth = reach;
        Ok(Block {
            stmts,
            span: open.span.to(close.span),
        })
    }

    fn stmt(&mut self) -> Result<Stmt, Diagnostic> {
        let open = self.labels.len();
        let stmt = self.stmt_opening_labels();
        // A loop label closes with its loop's body, also when the loop fails to parse.
        self.labels.truncate(open);
        stmt
    }

    /// A statement, with the loop label it opens still open.
    fn stmt_opening_labels(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.peek().span;
        let base = self.depth;
        let mut reach = base;
        if let Some(error) = self.label_not_on_loop() {
            return Err(error);
        }
        // A labeled `loop` is an expression, which `primary` reads with its loop label.
        let label = match self.after_label() {
            Some((_, TokenKind::Loop)) => None,
            _ => self.loop_label(),
        };
        let kind = match self.peek().kind {
            TokenKind::Let => self.let_stmt()?,
            TokenKind::Const => return Err(self.local_const()),
            TokenKind::While => {
                let keyword = self.bump().span;
                if let Some((pattern, value)) = self.child(base, &mut reach, Self::let_binding)? {
                    StmtKind::WhileLet {
                        label,
                        keyword,
                        pattern,
                        value,
                        body: self.child(base, &mut reach, Self::block)?,
                    }
                } else {
                    let cond = self.child(base, &mut reach, Self::expr)?;
                    StmtKind::While {
                        label,
                        keyword,
                        cond,
                        body: self.child(base, &mut reach, Self::block)?,
                    }
                }
            }
            TokenKind::For => {
                let keyword = self.bump().span;
                let name = self.binding()?;
                self.expect(&TokenKind::In)?;
                // A range is only valid here, so the header reads its two sides itself, counted
                // like `expr` does.
                let side = |p: &mut Self| {
                    p.descend()?;
                    p.binary(1)
                };
                let first = self.child(base, &mut reach, side)?;
                let source = match self.range_op() {
                    None => ForSource::Array(first),
                    Some(RangeOp::Foreign { at, lt }) => {
                        // Reported even when the end does not parse.
                        let end = self.child(base, &mut reach, side).ok().map(|e| e.span);
                        return Err(self.foreign_range(at, lt, first.span, end, true));
                    }
                    Some(op) => ForSource::Range {
                        start: first,
                        end: self.child(base, &mut reach, side)?,
                        inclusive: matches!(op, RangeOp::Inclusive),
                    },
                };
                StmtKind::For {
                    label,
                    keyword,
                    name,
                    source,
                    body: self.child(base, &mut reach, Self::block)?,
                }
            }
            TokenKind::Return => {
                self.bump();
                if self.at(&TokenKind::Newline) || self.at(&TokenKind::RBrace) {
                    StmtKind::Return(None)
                } else {
                    StmtKind::Return(Some(self.child(base, &mut reach, Self::expr)?))
                }
            }
            TokenKind::Break => {
                self.bump();
                let label = self.jump_label(false);
                if let Some(error) = label.as_ref().and_then(|label| self.label_clash(label)) {
                    return Err(error);
                }
                let value = if self.jump_ends() {
                    None
                } else {
                    Some(self.child(base, &mut reach, Self::expr)?)
                };
                StmtKind::Break { label, value }
            }
            TokenKind::Continue => {
                self.bump();
                StmtKind::Continue {
                    label: self.jump_label(true),
                }
            }
            TokenKind::LBrace => return Err(DiagnosticKind::BlockAsStatement { at: start }.into()),
            _ => self.expr_or_assign()?,
        };
        // `let_stmt` and `expr_or_assign` leave their own height in `self.depth`.
        self.depth = reach.max(self.depth);
        Ok(Stmt {
            kind,
            span: start.to(self.last_span()),
        })
    }

    /// The name and the token after it when the next tokens are `name:`, as before a loop label.
    fn after_label(&self) -> Option<(Symbol, &'a TokenKind)> {
        let [first, colon, next, ..] = &self.tokens[self.pos..] else {
            return None;
        };
        match first.kind {
            TokenKind::Ident(name) if colon.kind == TokenKind::Colon => Some((name, &next.kind)),
            _ => None,
        }
    }

    /// The loop label `name:` at the next token when `loop`, `while` or `for` follows it,
    /// consumed and opened, so that a jump in the loop's header or body can name it.
    fn loop_label(&mut self) -> Option<Ident> {
        let (name, TokenKind::Loop | TokenKind::While | TokenKind::For) = self.after_label()?
        else {
            return None;
        };
        let span = self.bump().span;
        self.bump();
        let label = Ident { name, span };
        self.labels.push(label.clone());
        Some(label)
    }

    /// The error of a `name:` before `if`, `match` or `{`, or of a `_:` before a loop, where a
    /// loop with its loop label could stand: at the start of a statement, after the `=` of a
    /// `let` or an assignment, and after an argument label. Any other `name:` is no loop label.
    fn label_not_on_loop(&self) -> Option<Diagnostic> {
        let [first, colon, next, ..] = &self.tokens[self.pos..] else {
            return None;
        };
        if colon.kind != TokenKind::Colon {
            return None;
        }
        let at = first.span.to(colon.span);
        // `{ a: { b: 1 } }` is an object literal to its writer: a block that opens with a
        // `name:` before no loop is an object's value, so `a:` keeps E0011.
        let mut inside = self.tokens[self.pos + 3..]
            .iter()
            .filter(|t| t.kind != TokenKind::Newline);
        let key = next.kind == TokenKind::LBrace
            && matches!(
                (inside.next(), inside.next(), inside.next()),
                (Some(name), Some(colon), Some(after))
                    if matches!(name.kind, TokenKind::Ident(_))
                        && colon.kind == TokenKind::Colon
                        && !matches!(after.kind, TokenKind::Loop | TokenKind::While | TokenKind::For)
            );
        match (&first.kind, &next.kind) {
            (TokenKind::Ident(_), TokenKind::If | TokenKind::Match | TokenKind::LBrace) if !key => {
                Some(
                    DiagnosticKind::LabelNotOnLoop {
                        at,
                        next: next.span,
                    }
                    .into(),
                )
            }
            (TokenKind::Underscore, TokenKind::Loop | TokenKind::While | TokenKind::For) => Some(
                DiagnosticKind::UnnamedLabel {
                    at,
                    remove: Span::new(at.start(), next.span.start()),
                }
                .into(),
            ),
            _ => None,
        }
    }

    /// The error of the loop label `label` after `break` when the next token would make it more
    /// than a loop label: a `(` or `[` that touches it, or a token that can only continue an
    /// expression.
    fn label_clash(&self, label: &Ident) -> Option<Diagnostic> {
        let next = self.peek();
        let touches = next.span.start() == label.span.end();
        let form = match next.kind {
            TokenKind::LParen if touches => ClashForm::Call {
                bracket: next.span,
                text: self.bracketed(&TokenKind::LParen, &TokenKind::RParen),
            },
            TokenKind::LBracket if touches => ClashForm::Index {
                bracket: next.span,
                text: self.bracketed(&TokenKind::LBracket, &TokenKind::RBracket),
            },
            TokenKind::Plus
            | TokenKind::Star
            | TokenKind::StarStar
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::EqEq
            | TokenKind::NotEq
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::AndAnd
            | TokenKind::OrOr
            | TokenKind::Is
            | TokenKind::QuestionQuestion
            | TokenKind::Dot
            | TokenKind::QuestionDot => ClashForm::Operator { next: next.span },
            _ => return None,
        };
        let declared = self
            .labels
            .iter()
            .rev()
            .find(|open| open.name == label.name)
            .expect("a `break` label should be an open loop label")
            .span;
        Some(
            DiagnosticKind::LabelClash {
                name: label.name.to_string(),
                at: label.span,
                declared,
                form,
            }
            .into(),
        )
    }

    /// The source text from the `open` bracket at the next token through its matching `close`,
    /// or of the bracket alone when the lexer found none.
    fn bracketed(&self, open: &TokenKind, close: &TokenKind) -> String {
        let start = self.peek().span;
        let mut depth = 0usize;
        let end = self.tokens[self.pos..]
            .iter()
            .find(|token| {
                if token.kind == *open {
                    depth += 1;
                } else if token.kind == *close {
                    depth -= 1;
                }
                depth == 0
            })
            .map_or(start, |token| token.span);
        self.source[start.to(end).range()].to_string()
    }

    /// The loop label after `break` or `continue`: a name, when it is an open loop label or
    /// `any` is set.
    fn jump_label(&mut self, any: bool) -> Option<Ident> {
        let TokenKind::Ident(name) = self.peek().kind else {
            return None;
        };
        (any || self.labels.iter().any(|label| label.name == name)).then(|| Ident {
            name,
            span: self.bump().span,
        })
    }

    /// Whether the next token ends a `break` or `continue`, which then takes no value: only a
    /// token that can start an expression continues one.
    fn jump_ends(&self) -> bool {
        matches!(
            self.peek().kind,
            TokenKind::Newline
                | TokenKind::Semicolon
                | TokenKind::RBrace
                | TokenKind::Comma
                | TokenKind::RParen
                | TokenKind::RBracket
                | TokenKind::FatArrow
                | TokenKind::Eof
        )
    }

    /// The `else` that starts a line, after `previous`, the statement before it. The `else` can
    /// move up to the `if` it follows when that has none, and a block or an `if` follows it.
    fn else_on_new_line(&self, previous: Option<&Stmt>) -> Diagnostic {
        let at = self.peek().span;
        let lacks_else = previous.is_some_and(|stmt| {
            matches!(
                &stmt.kind,
                StmtKind::Expr(Expr {
                    kind: ExprKind::If {
                        else_block: None,
                        ..
                    } | ExprKind::IfLet {
                        else_block: None,
                        ..
                    },
                    ..
                })
            )
        });
        let takes_block = matches!(
            self.tokens[self.pos + 1].kind,
            TokenKind::LBrace | TokenKind::If
        );
        // `lacks_else` means the statement ends with the `}` just before the `else` and any
        // line breaks. The join deletes the gap, so a comment or a dropped `;` in it, which has
        // its own W0007, makes the join unsafe.
        let gap = self.last_code_end()..at.start();
        let join = (lacks_else && takes_block).then(|| Join {
            gap: Span::new(gap.start, gap.end),
            safe: self.source[gap.clone()].trim().is_empty(),
        });
        DiagnosticKind::ElseOnNewLine { at, join }.into()
    }

    /// Where the code before the next token ends: the `}` of the statement before it.
    fn last_code_end(&self) -> usize {
        self.tokens[..self.pos]
            .iter()
            .rfind(|t| t.kind != TokenKind::Newline)
            .map_or(0, |t| t.span.end())
    }

    /// The span of the token just consumed.
    fn last_span(&self) -> Span {
        self.tokens[self.pos - 1].span
    }

    /// `let pattern = value`, after `if` or `while`; `None` when no `let` follows.
    fn let_binding(&mut self) -> Result<Option<(Pattern, Expr)>, Diagnostic> {
        if self.eat(&TokenKind::Let).is_none() {
            return Ok(None);
        }
        let base = self.depth;
        let mut reach = base;
        let pattern = self.child(base, &mut reach, Self::pattern)?;
        self.expect(&TokenKind::Eq)?;
        let value = self.child(base, &mut reach, Self::expr)?;
        self.depth = reach;
        Ok(Some((pattern, value)))
    }

    /// A pattern: one or more `single`s joined by `|`.
    fn pattern(&mut self) -> Result<Pattern, Diagnostic> {
        self.descend()?;
        let base = self.depth;
        let mut reach = base;
        let first = self.child(base, &mut reach, Self::single)?;
        let pattern = if self.at(&TokenKind::Pipe) {
            let mut span = first.span;
            let mut alternatives = vec![first];
            while self.eat(&TokenKind::Pipe).is_some() {
                let next = self.child(base, &mut reach, Self::single)?;
                span = span.to(next.span);
                alternatives.push(next);
            }
            // The `Or` is the parent of its alternatives: one level more.
            self.reach(reach + 1)?;
            return Ok(Pattern {
                kind: PatternKind::Or(alternatives),
                span,
            });
        } else {
            first
        };
        self.depth = reach;
        Ok(pattern)
    }

    /// One alternative of a pattern: `_`, `None`, a name, `Num(p, ...)`, `Expr.Num(p, ...)`,
    /// `Expr.Idle`, a literal, or a range of two literals. The caller has counted its level.
    fn single(&mut self) -> Result<Pattern, Diagnostic> {
        let base = self.depth;
        let mut reach = base;
        let token = self.peek();
        let (kind, span) = match token.kind {
            TokenKind::Minus
            | TokenKind::Int(_)
            | TokenKind::Char(_)
            | TokenKind::Str(_)
            | TokenKind::True
            | TokenKind::False => return self.literal_pattern(),
            TokenKind::Underscore => {
                self.bump();
                (PatternKind::Wild, token.span)
            }
            TokenKind::NoneKw => {
                self.bump();
                (PatternKind::None, token.span)
            }
            TokenKind::Ident(name) => {
                let first = Ident {
                    name,
                    span: self.bump().span,
                };
                if let Some(error) = self.path_idiom(first.span) {
                    return Err(error);
                }
                let (qualifier, name) = if self.eat(&TokenKind::Dot).is_some() {
                    (Some(first), self.ident("a variant name")?)
                } else {
                    (None, first)
                };
                // The pattern as written up to the end of the variant name.
                let path = Span::new(token.span.start(), name.span.end());
                if self.at(&TokenKind::LParen) {
                    let (fields, labels, parens) =
                        self.sub_patterns(base, &mut reach, &TokenKind::RParen)?;
                    if let Some(&(first_label, _)) = labels.first() {
                        return Err(DiagnosticKind::PatternForm {
                            form: ForeignPattern::Labels {
                                form: format!(
                                    "{}{}",
                                    &self.source[path.range()],
                                    self.listed(&fields)
                                ),
                                labels: labels.into_iter().map(|(_, remove)| remove).collect(),
                            },
                            at: first_label,
                        }
                        .into());
                    }
                    let span = token.span.to(parens);
                    let kind = PatternKind::Variant {
                        qualifier,
                        name,
                        fields,
                        parens: Some(parens),
                    };
                    (kind, span)
                } else if self.at(&TokenKind::LBrace) {
                    // Rust's `Rect { w, h }`, or with labels, `Rect { w: x, h: y }`.
                    let (fields, _, braces) =
                        self.sub_patterns(base, &mut reach, &TokenKind::RBrace)?;
                    let fields = self.listed(&fields);
                    return Err(DiagnosticKind::PatternForm {
                        form: ForeignPattern::Braces {
                            form: format!("{}{fields}", &self.source[path.range()]),
                            replace: Span::new(path.end(), braces.end()),
                            fields,
                        },
                        at: braces,
                    }
                    .into());
                } else if qualifier.is_some() {
                    let span = token.span.to(name.span);
                    let kind = PatternKind::Variant {
                        qualifier,
                        name,
                        fields: Vec::new(),
                        parens: None,
                    };
                    (kind, span)
                } else if let Some(op) = self.range_op() {
                    return self.range_end(Bound::Name(name), token.span, op);
                } else {
                    (PatternKind::Name(name), token.span)
                }
            }
            TokenKind::StrHead(_) => return Err(self.interpolated_pattern()),
            _ => return Err(self.expected("a pattern")),
        };
        // A name or a literal can start a range; no other pattern is a bound.
        if matches!(self.peek().kind, TokenKind::DotDot | TokenKind::DotDotEq) {
            return Err(self.expected_at(token, "a literal"));
        }
        self.depth = reach;
        Ok(Pattern { kind, span })
    }

    /// The error for a string pattern with an interpolation, at the next token, the string's
    /// head: `InterpolatedPattern` on the first interpolation, once it parses.
    fn interpolated_pattern(&mut self) -> Diagnostic {
        // The head's text ends just after its `{`, and the next piece starts at its `}`.
        let open = self.bump().span.end() - 1;
        let base = self.depth;
        let mut reach = base;
        if let Err(error) = self.child(base, &mut reach, Self::expr) {
            return error;
        }
        if matches!(self.peek().kind, TokenKind::FormatSpec(_)) {
            self.bump();
        }
        let close = self.peek().span.start();
        if !matches!(
            self.peek().kind,
            TokenKind::StrMiddle(_) | TokenKind::StrTail(_)
        ) {
            return self.expected("`}`");
        }
        DiagnosticKind::InterpolatedPattern {
            at: Span::new(open, close + 1),
        }
        .into()
    }

    /// A literal pattern, or a range that starts with one, at the next token.
    fn literal_pattern(&mut self) -> Result<Pattern, Diagnostic> {
        let (start, start_span) = self.literal()?;
        match self.range_op() {
            Some(op) => self.range_end(Bound::Literal(start), start_span, op),
            None => Ok(Pattern {
                kind: PatternKind::Literal(start),
                span: start_span,
            }),
        }
    }

    /// The rest of a range pattern, after its `start` at `start_span` and its operator `op`: its
    /// end. A bare name is a bound the checker reports, but not when it starts a path or a call;
    /// nothing else but a literal is a bound. A range written another language's way is reported
    /// even when its end does not parse.
    fn range_end(
        &mut self,
        start: Bound,
        start_span: Span,
        op: RangeOp,
    ) -> Result<Pattern, Diagnostic> {
        let bound = |p: &mut Self| {
            let token = p.peek();
            match token.kind {
                TokenKind::Ident(name)
                    if !matches!(p.tokens[p.pos + 1].kind, TokenKind::LParen | TokenKind::Dot) =>
                {
                    p.bump();
                    Ok((
                        Bound::Name(Ident {
                            name,
                            span: token.span,
                        }),
                        token.span,
                    ))
                }
                _ => p
                    .literal()
                    .map(|(literal, span)| (Bound::Literal(literal), span)),
            }
        };
        let end = bound(self);
        if let RangeOp::Foreign { at, lt } = op {
            let end = end.ok().map(|(_, span)| span);
            return Err(self.foreign_range(at, lt, start_span, end, true));
        }
        let (end, end_span) = end?;
        Ok(Pattern {
            kind: PatternKind::Range {
                start,
                end,
                inclusive: matches!(op, RangeOp::Inclusive),
            },
            span: start_span.to(end_span),
        })
    }

    /// The literal at the next token, `["-"] Int | Char | Str | "true" | "false"`, and its
    /// span, `-` included. Anything else is "expected a literal".
    fn literal(&mut self) -> Result<(Literal, Span), Diagnostic> {
        let token = self.peek();
        let minus = (token.kind == TokenKind::Minus).then(|| self.bump().span);
        let token = self.peek();
        let literal = match (&token.kind, minus) {
            (&TokenKind::Int(magnitude), _) => {
                // Only a `-` takes `i64::MIN`'s magnitude, as in an expression.
                let value = if minus.is_some() && magnitude == MIN_MAGNITUDE {
                    i64::MIN
                } else {
                    let value =
                        i64::try_from(magnitude).map_err(|_| literal_too_large(token.span))?;
                    if minus.is_some() { -value } else { value }
                };
                Literal::Int {
                    value,
                    text: token.span,
                    negative: minus,
                }
            }
            (_, Some(_)) => return Err(self.expected("an integer literal")),
            (&TokenKind::Char(c), None) => Literal::Char(c),
            (TokenKind::Str(s), None) => Literal::Str(s.clone()),
            (&TokenKind::True | &TokenKind::False, None) => {
                Literal::Bool(token.kind == TokenKind::True)
            }
            _ => return Err(self.expected("a literal")),
        };
        let end = self.bump().span;
        Ok((literal, minus.map_or(end, |minus| minus.to(end))))
    }

    /// The range operator at the next token, consumed: `..`, `..=`, or another language's
    /// `..<` and `...` when `..` is followed with no gap by a `<` or a `.`.
    fn range_op(&mut self) -> Option<RangeOp> {
        let op = self.peek();
        match op.kind {
            TokenKind::DotDotEq => {
                self.bump();
                Some(RangeOp::Inclusive)
            }
            TokenKind::DotDot => {
                self.bump();
                let next = self.peek();
                if next.span.start() == op.span.end()
                    && matches!(next.kind, TokenKind::Lt | TokenKind::Dot)
                {
                    self.bump();
                    return Some(RangeOp::Foreign {
                        at: op.span.to(next.span),
                        lt: (next.kind == TokenKind::Lt).then_some(next.span),
                    });
                }
                Some(RangeOp::Exclusive)
            }
            _ => None,
        }
    }

    /// The error for a range written another language's way: `at` is its operator, `lt` the
    /// `<` of `..<`, and `start` and `end` its bounds, `end` being `None` when it did not parse.
    /// Removing the `<` is a safe fix only where `fixable` and the end parsed: where the range is
    /// not allowed, or the end is broken, it leaves an error.
    fn foreign_range(
        &self,
        at: Span,
        lt: Option<Span>,
        start: Span,
        end: Option<Span>,
        fixable: bool,
    ) -> Diagnostic {
        let start_text = self.source[start.range()].to_string();
        let end_text = end.map(|end| self.source[end.range()].to_string());
        let form = match lt {
            Some(lt) => ForeignRange::Exclusive {
                start: start_text,
                end: end_text,
                remove: (fixable && end.is_some()).then_some(lt),
            },
            None => ForeignRange::Ellipsis {
                start: start_text,
                end: end_text,
            },
        };
        DiagnosticKind::ForeignRange { form, at }.into()
    }

    /// A lone `|` where an expression stands: logical or is `||`.
    fn lone_pipe(&self) -> Diagnostic {
        let at = self.peek().span;
        DiagnosticKind::UnexpectedCharacter { c: '|', at }.into()
    }

    /// The sub-patterns after the `(` or `{` at the next token, through `close`; the label
    /// before each one that has it, with the span its fix deletes; and the span from the opening
    /// token through `close`.
    #[expect(
        clippy::type_complexity,
        reason = "the one caller of each part takes it apart at once"
    )]
    fn sub_patterns(
        &mut self,
        base: usize,
        reach: &mut usize,
        close: &TokenKind,
    ) -> Result<(Vec<Pattern>, Vec<(Span, Span)>, Span), Diagnostic> {
        let open = self.bump().span;
        let mut fields = Vec::new();
        let mut labels = Vec::new();
        while !self.at(close) {
            if let Some(label) = self.pattern_label() {
                labels.push(label);
            }
            fields.push(self.child(base, reach, Self::pattern)?);
            if self.eat(&TokenKind::Comma).is_none() {
                break;
            }
        }
        let close = self.expect(close)?.span;
        Ok((fields, labels, open.to(close)))
    }

    /// Skips a `label:` before a sub-pattern, and returns the label's span and the span from it
    /// to the sub-pattern. `Name::` is no label: `pattern` reports the path.
    fn pattern_label(&mut self) -> Option<(Span, Span)> {
        let label = self.peek();
        // A name is not `Eof`, so the token after it exists, and so does the one after a `:`.
        if !matches!(label.kind, TokenKind::Ident(_))
            || self.tokens[self.pos + 1].kind != TokenKind::Colon
        {
            return None;
        }
        let (colon, next) = (&self.tokens[self.pos + 1], &self.tokens[self.pos + 2]);
        if next.kind == TokenKind::Colon && next.span.start() == colon.span.end() {
            return None;
        }
        self.pos += 2;
        Some((label.span, Span::new(label.span.start(), next.span.start())))
    }

    /// `patterns` as written, in parentheses: `(x, y)`.
    fn listed(&self, patterns: &[Pattern]) -> String {
        let written: Vec<&str> = patterns
            .iter()
            .map(|p| &self.source[p.span.range()])
            .collect();
        format!("({})", written.join(", "))
    }

    /// C++'s and Rust's `Name::member` at the next token, after the name at `name`.
    fn path_idiom(&self, name: Span) -> Option<Diagnostic> {
        let first = self.peek();
        if first.kind != TokenKind::Colon {
            return None;
        }
        // A `:` is not `Eof`, so the token after each exists.
        let second = &self.tokens[self.pos + 1];
        if second.kind != TokenKind::Colon || second.span.start() != first.span.end() {
            return None;
        }
        let member = &self.tokens[self.pos + 2];
        let TokenKind::Ident(_) = member.kind else {
            return None;
        };
        Some(
            DiagnosticKind::MatchIdiom {
                form: MatchIdiomForm::Path {
                    name: self.source[name.range()].to_string(),
                    member: self.source[member.span.range()].to_string(),
                },
                at: first.span.to(second.span),
            }
            .into(),
        )
    }

    /// The error where more follows `stmt` on its line. A statement that ends in C's `switch`
    /// gets its own error: a call `switch(e)` and then `{`, or a name `switch` and then an
    /// expression and `{`. That expression is parsed only here, where the line fails anyway.
    fn statement_end(&mut self, stmt: &Stmt) -> Diagnostic {
        let error = self.end_of_line();
        let (StmtKind::Expr(tail)
        | StmtKind::Return(Some(tail))
        | StmtKind::Let { init: tail, .. }
        | StmtKind::Assign { value: tail, .. }
        | StmtKind::CompoundAssign { value: tail, .. }) = &stmt.kind
        else {
            return error;
        };
        let switch = match &tail.kind {
            ExprKind::Name(name) if name.as_str() == "switch" => {
                let specs = self.specs.len();
                let expr = self.expr().is_ok();
                self.specs.truncate(specs);
                expr.then_some(tail.span)
            }
            ExprKind::Call { callee, .. } if callee.name.as_str() == "switch" => Some(callee.span),
            _ => None,
        };
        match switch {
            Some(at) if self.at(&TokenKind::LBrace) => DiagnosticKind::MatchIdiom {
                form: MatchIdiomForm::Switch,
                at,
            }
            .into(),
            _ => error,
        }
    }

    /// `case`, `default:` or `else =>` where an arm starts.
    fn arm_idiom(&self) -> Option<Diagnostic> {
        let token = self.peek();
        if !matches!(token.kind, TokenKind::Ident(_) | TokenKind::Else) {
            return None;
        }
        // Neither is `Eof`, so the token after it exists.
        let next = &self.tokens[self.pos + 1];
        let text = &self.source[token.span.range()];
        let (form, at) = match (&token.kind, &next.kind) {
            (TokenKind::Ident(_), kind) if text == "case" && *kind != TokenKind::FatArrow => {
                // Swift's `case let .some(x)` also loses its `let` and its `.`. A `let` is not
                // `Eof`, so the token after it exists.
                let mut remove = vec![Span::new(token.span.start(), next.span.start())];
                let mut after = next;
                if next.kind == TokenKind::Let {
                    after = &self.tokens[self.pos + 2];
                    remove.push(Span::new(next.span.start(), after.span.start()));
                }
                if after.kind == TokenKind::Dot {
                    remove.push(after.span);
                }
                (MatchIdiomForm::Case { remove }, token.span)
            }
            (TokenKind::Ident(_), TokenKind::Colon) if text == "default" => {
                (MatchIdiomForm::Default, token.span.to(next.span))
            }
            (TokenKind::Else, TokenKind::FatArrow) => (MatchIdiomForm::Else, token.span),
            _ => return None,
        };
        Some(DiagnosticKind::MatchIdiom { form, at }.into())
    }

    /// `match scrutinee {`, then one arm per line, then `}`.
    fn match_expr(&mut self) -> Result<Expr, Diagnostic> {
        let keyword = self.bump().span;
        let base = self.depth;
        let mut reach = base;
        let scrutinee = self.child(base, &mut reach, Self::expr)?;
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();
        let mut arms = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            arms.push(self.child(base, &mut reach, Self::arm)?);
            if self.at(&TokenKind::Comma) {
                return Err(self.member_comma(diagnostic::Member::Arm));
            }
            if !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Newline) {
                return Err(self.end_of_line());
            }
            self.skip_newlines();
        }
        let close = self.bump();
        self.depth = reach;
        Ok(Expr {
            span: keyword.to(close.span),
            kind: ExprKind::Match {
                keyword,
                scrutinee: Box::new(scrutinee),
                arms,
                close: close.span,
            },
        })
    }

    /// `pattern => body`, or `pattern if guard => body`.
    fn arm(&mut self) -> Result<Arm, Diagnostic> {
        let base = self.depth;
        let mut reach = base;
        if let Some(error) = self.arm_idiom() {
            return Err(error);
        }
        let pattern = self.child(base, &mut reach, Self::pattern)?;
        self.arm_split(&[TokenKind::Pipe, TokenKind::FatArrow, TokenKind::If])?;
        let guard = if self.eat(&TokenKind::If).is_some() {
            let guard = self.child(base, &mut reach, Self::expr)?;
            self.arm_split(&[TokenKind::FatArrow])?;
            Some(guard)
        } else {
            None
        };
        if self.at(&TokenKind::Arrow) {
            return Err(DiagnosticKind::MatchIdiom {
                form: MatchIdiomForm::Arrow,
                at: self.peek().span,
            }
            .into());
        }
        self.expect(&TokenKind::FatArrow)?;
        let body = self.child(base, &mut reach, Self::arm_body)?;
        self.depth = reach;
        Ok(Arm {
            pattern,
            guard,
            body,
        })
    }

    /// `ArmSplit` when the line ends here and the next line with code starts with one of
    /// `continues`, so continues the arm. The fix joins the lines, unless a comment, or a dropped
    /// `;` with its own W0007, lies between.
    /// A `;` that separates ends the arm on purpose, so the parser reports what follows it.
    fn arm_split(&self, continues: &[TokenKind]) -> Result<(), Diagnostic> {
        if !self.at(&TokenKind::Newline) || !self.peek().span.range().is_empty() {
            return Ok(());
        }
        let Some(next) = self.tokens[self.pos..]
            .iter()
            .find(|t| t.kind != TokenKind::Newline)
            .filter(|t| continues.contains(&t.kind))
        else {
            return Ok(());
        };
        let gap = self.last_code_end()..next.span.start();
        let join = self.source[gap.clone()]
            .trim()
            .is_empty()
            .then(|| Span::new(gap.start, gap.end));
        Err(DiagnosticKind::ArmSplit {
            at: next.span,
            join,
        }
        .into())
    }

    /// An arm's body: an expression, or a statement that is not one (an assignment, `return`,
    /// `break` or `continue`) in a block holding just it.
    fn arm_body(&mut self) -> Result<Expr, Diagnostic> {
        let start = self.peek().span;
        let stmt = if matches!(
            self.peek().kind,
            TokenKind::Return | TokenKind::Break | TokenKind::Continue
        ) {
            self.stmt()?
        } else {
            // `primary` reports a `_` body without the `let _ =` fix, which no arm accepts.
            let kind = if self.at(&TokenKind::Underscore) {
                StmtKind::Expr(self.expr()?)
            } else {
                self.expr_or_assign()?
            };
            Stmt {
                kind,
                span: start.to(self.last_span()),
            }
        };
        let span = stmt.span;
        if let Stmt {
            kind: StmtKind::Expr(e),
            ..
        } = stmt
        {
            return Ok(e);
        }
        // The block is one level above the statement it holds.
        self.reach(self.depth + 1)?;
        Ok(Expr {
            kind: ExprKind::Block(Block {
                stmts: vec![stmt],
                span,
            }),
            span,
        })
    }

    fn let_stmt(&mut self) -> Result<StmtKind, Diagnostic> {
        let base = self.depth;
        let mut reach = base;
        self.bump();
        let mutable = self.eat(&TokenKind::Mut).map(|t| t.span);
        let name = self.binding()?;
        let ty = match self.eat(&TokenKind::Colon) {
            Some(_) => Some(self.child(base, &mut reach, Self::parse_type)?),
            None => None,
        };
        self.expect(&TokenKind::Eq)?;
        if let Some(error) = self.label_not_on_loop() {
            return Err(error);
        }
        let init = self.child(base, &mut reach, Self::expr)?;
        self.depth = reach;
        // Reported last, and only where the statement ends, so that the fix leaves a `let` that
        // parses: past a stray token, the block reports that token's own error.
        if let Some(keyword) = mutable
            && name.name.as_str() == "_"
            && (self.at(&TokenKind::Newline) || self.at(&TokenKind::RBrace))
        {
            return Err(DiagnosticKind::MutOnUnderscore {
                at: name.span,
                keyword,
            }
            .into());
        }
        Ok(StmtKind::Let {
            mutable,
            name,
            ty,
            init,
        })
    }

    /// The error of a `const` statement. It is reported only once the statement parses as a `let`
    /// and ends the line, so that its fix leaves a `let` that parses; before that, the first
    /// error in it is reported.
    fn local_const(&mut self) -> Diagnostic {
        let at = self.peek().span;
        if let Err(error) = self.let_stmt() {
            return error;
        }
        if !self.at(&TokenKind::Newline) && !self.at(&TokenKind::RBrace) {
            return self.end_of_line();
        }
        DiagnosticKind::ConstInFunction { at }.into()
    }

    /// An expression statement, or an assignment when an assignment operator follows.
    fn expr_or_assign(&mut self) -> Result<StmtKind, Diagnostic> {
        if self.at(&TokenKind::Underscore) {
            return Err(self.underscore_statement());
        }
        let base = self.depth;
        let mut reach = base;
        let target = self.child(base, &mut reach, Self::expr)?;
        let op = match self.peek().kind {
            TokenKind::Eq => None,
            TokenKind::PlusEq => Some(BinaryOp::Add),
            TokenKind::MinusEq => Some(BinaryOp::Sub),
            TokenKind::StarEq => Some(BinaryOp::Mul),
            TokenKind::SlashEq => Some(BinaryOp::Div),
            TokenKind::PercentEq => Some(BinaryOp::Rem),
            _ => return Ok(StmtKind::Expr(target)),
        };
        if matches!(target.kind, ExprKind::SelfValue) {
            return Err(DiagnosticKind::AssignToSelf { at: target.span }.into());
        }
        if !is_place(&target) {
            return Err(DiagnosticKind::AssignToExpression { at: target.span }.into());
        }
        if goes_through_safe_access(&target) {
            return Err(DiagnosticKind::AssignThroughSafeAccess { at: target.span }.into());
        }
        let op_span = self.bump().span;
        if let Some(error) = self.label_not_on_loop() {
            return Err(error);
        }
        let value = self.child(base, &mut reach, Self::expr)?;
        self.depth = reach;
        Ok(match op {
            None => StmtKind::Assign { target, value },
            Some(op) => StmtKind::CompoundAssign {
                op,
                op_span,
                target,
                value,
            },
        })
    }

    /// The error of a statement that starts with `_`. A plain `_ = e` whose value parses and ends
    /// the line gets the fix `let _ = e`; a value that fails to parse reports its own error.
    fn underscore_statement(&mut self) -> Diagnostic {
        let at = self.bump().span;
        let mut bind = false;
        if self.eat(&TokenKind::Eq).is_some() {
            if let Err(error) = self.expr() {
                return error;
            }
            bind = matches!(self.peek().kind, TokenKind::Newline | TokenKind::RBrace);
        }
        DiagnosticKind::UnderscoreValue { at, bind }.into()
    }

    fn expr(&mut self) -> Result<Expr, Diagnostic> {
        self.descend()?;
        let e = self.binary(1)?;
        if matches!(self.peek().kind, TokenKind::DotDot | TokenKind::DotDotEq) {
            let at = self.peek().span;
            return Err(match self.range_op() {
                Some(RangeOp::Foreign { at, lt }) => {
                    let end = self.binary(1).ok().map(|end| end.span);
                    self.foreign_range(at, lt, e.span, end, false)
                }
                _ => DiagnosticKind::RangeOutsideFor { at }.into(),
            });
        }
        Ok(e)
    }

    /// Precedence climbing over levels 1 to 7; `min` is the lowest level to accept.
    /// Each operator adds a level: `1 + 1 + 1` builds a tree that is as deep as it is long.
    fn binary(&mut self, min: u8) -> Result<Expr, Diagnostic> {
        let base = self.depth;
        let mut lhs = self.unary()?;
        loop {
            let kind = &self.peek().kind;
            if compare_op(kind).is_some() {
                if min > 4 {
                    return Ok(lhs);
                }
                lhs = self.comparison_chain(lhs, base)?;
            } else if *kind == TokenKind::StarStar {
                // `**` binds as `*` does.
                if POWER_LEVEL < min {
                    return Ok(lhs);
                }
                return Err(self.power_operator(&lhs, base));
            } else if let Some((level, op)) = infix_op(kind) {
                if level < min {
                    return Ok(lhs);
                }
                // The operator is the parent of everything parsed so far: one level more.
                let wrapped = self.depth + 1;
                self.reach(wrapped)?;
                let op_span = self.bump().span;
                self.depth = base + 1;
                // `??` is right-associative: its right side starts at its own level.
                let rhs = self.binary(if matches!(op, Infix::Coalesce) {
                    level
                } else {
                    level + 1
                })?;
                self.depth = self.depth.max(wrapped);
                let span = lhs.span.to(rhs.span);
                let (lhs_box, rhs_box) = (Box::new(lhs), Box::new(rhs));
                lhs = Expr {
                    span,
                    kind: match op {
                        Infix::Binary(op) => ExprKind::Binary {
                            op,
                            op_span,
                            lhs: lhs_box,
                            rhs: rhs_box,
                        },
                        Infix::Is => ExprKind::Is {
                            lhs: lhs_box,
                            rhs: rhs_box,
                        },
                        Infix::Coalesce => ExprKind::Coalesce {
                            lhs: lhs_box,
                            rhs: rhs_box,
                        },
                    },
                };
                // Level 3 is non-associative: a second `==`/`!=`/`is` is an error, not a chain.
                if level == 3
                    && matches!(
                        self.peek().kind,
                        TokenKind::Is | TokenKind::EqEq | TokenKind::NotEq
                    )
                {
                    return Err(DiagnosticKind::EqualityChain {
                        at: self.peek().span,
                    }
                    .into());
                }
            } else if *kind == TokenKind::Pipe {
                return Err(self.lone_pipe());
            } else {
                return Ok(lhs);
            }
        }
    }

    /// The error for the `**` that is next, after `lhs`: the right side parses as `*`'s would,
    /// and a `=` straight after the `**` is part of the error, so `x **= 2` is reported once.
    /// The unsafe fix is `lhs.pow(rhs)`: offered when both sides are plain operands, and not
    /// for `**=` or a chain of `**`, which Python reads from the right.
    fn power_operator(&mut self, lhs: &Expr, base: usize) -> Diagnostic {
        let left_ok = power_operand(lhs, self.operand_tokens(lhs), false);
        let at = self.bump().span;
        let assign = self.eat(&TokenKind::Eq).is_some();
        self.depth = base + 1;
        let rhs = match self.binary(POWER_LEVEL + 1) {
            Ok(rhs) => rhs,
            Err(error) => return error,
        };
        let fix = (left_ok
            && !assign
            && !self.at(&TokenKind::StarStar)
            && power_operand(&rhs, self.operand_tokens(&rhs), true))
        .then_some(PowerFix {
            left: lhs.span,
            right: rhs.span,
        });
        DiagnosticKind::PowerOperator { at, fix }.into()
    }

    /// The tokens of `e`, the expression just parsed: those back from the last consumed one to
    /// the first, which starts where `e` does.
    fn operand_tokens(&self, e: &Expr) -> &'a [Token] {
        let consumed = &self.tokens[..self.pos];
        let first = consumed
            .iter()
            .rposition(|t| t.span.start() == e.span.start())
            .expect("the first token of an expression just parsed should be consumed");
        &consumed[first..]
    }

    /// Level 4: `first` and every `<`/`<=`/`>`/`>=` operand after it, as one node.
    fn comparison_chain(&mut self, first: Expr, base: usize) -> Result<Expr, Diagnostic> {
        let start = first.span;
        // The chain is the parent of `first`: one level more.
        let mut reach = self.depth + 1;
        self.reach(reach)?;
        let mut operands = vec![first];
        let mut ops: Vec<CompareOp> = Vec::new();
        while let Some(op) = compare_op(&self.peek().kind) {
            if ops
                .first()
                .is_some_and(|first| first.is_less() != op.is_less())
            {
                return Err(DiagnosticKind::ComparisonDirection {
                    at: self.peek().span,
                }
                .into());
            }
            self.bump();
            ops.push(op);
            self.depth = base + 1;
            operands.push(self.binary(5)?);
            reach = reach.max(self.depth);
        }
        self.depth = reach;
        Ok(Expr {
            span: start.to(self.last_span()),
            kind: ExprKind::Compare { operands, ops },
        })
    }

    fn unary(&mut self) -> Result<Expr, Diagnostic> {
        let op = match self.peek().kind {
            TokenKind::Minus => UnaryOp::Neg,
            TokenKind::Bang => UnaryOp::Not,
            _ => return self.postfix(),
        };
        self.descend()?;
        let start = self.bump().span;
        if matches!(op, UnaryOp::Neg) && self.at_min_literal() {
            // `i64::MIN` has no positive twin, so `-9223372036854775808` is one literal.
            let literal = self.bump().span;
            return Ok(Expr {
                span: start.to(literal),
                kind: ExprKind::Int {
                    value: i64::MIN,
                    literal,
                },
            });
        }
        let operand = self.unary()?;
        Ok(Expr {
            span: start.to(operand.span),
            kind: ExprKind::Unary {
                op,
                operand: Box::new(operand),
            },
        })
    }

    /// Whether the next token is the magnitude of `i64::MIN` that nothing postfix follows. A `.`,
    /// `?.` or `[` would apply to the literal before the `-` does, so that literal is too large.
    fn at_min_literal(&self) -> bool {
        // `Int` is never `Eof`, so the token after it exists.
        matches!(self.peek().kind, TokenKind::Int(MIN_MAGNITUDE))
            && !starts_postfix(&self.tokens[self.pos + 1].kind)
    }

    /// `primary` followed by any number of `.field`, `.method(args)`, `?.field`,
    /// `?.method(args)` and `[index]`.
    fn postfix(&mut self) -> Result<Expr, Diagnostic> {
        let base = self.depth;
        let mut expr = self.primary()?;
        loop {
            // Each link is the parent of everything parsed so far: one level more.
            let wrapped = self.depth + 1;
            if self.at(&TokenKind::LBracket) {
                self.reach(wrapped)?;
                self.bump();
                self.depth = base + 1;
                let index = self.expr()?;
                let close = self.expect(&TokenKind::RBracket)?;
                self.depth = self.depth.max(wrapped);
                expr = Expr {
                    span: expr.span.to(close.span),
                    kind: ExprKind::Index {
                        base: Box::new(expr),
                        index: Box::new(index),
                    },
                };
                continue;
            }
            // `[` was handled above, so only `.` and `?.` reach here.
            if !starts_postfix(&self.peek().kind) {
                return Ok(expr);
            }
            self.reach(wrapped)?;
            let access = self.bump();
            let safe = access.kind == TokenKind::QuestionDot;
            let name = self.ident("a field or method name")?;
            expr = if self.at(&TokenKind::LParen) {
                self.depth = base + 1;
                let open = self.peek().span;
                let (args, close) = self.args()?;
                self.depth = self.depth.max(wrapped);
                Expr {
                    span: expr.span.to(close),
                    kind: ExprKind::MethodCall {
                        receiver: Box::new(expr),
                        method: name,
                        args,
                        parens: open.to(close),
                        safe,
                    },
                }
            } else {
                Expr {
                    span: expr.span.to(name.span),
                    kind: ExprKind::Field {
                        base: Box::new(expr),
                        name,
                        safe,
                    },
                }
            };
        }
    }

    /// `[` after the opening bracket: `]`, `value; len]` or `a, b, …]`.
    fn array(&mut self, open: Span) -> Result<Expr, Diagnostic> {
        let base = self.depth;
        let mut reach = base;
        let mut elements = Vec::new();
        let mut fill = None;
        while !self.at(&TokenKind::RBracket) {
            elements.push(self.child(base, &mut reach, Self::expr)?);
            if elements.len() == 1 && self.eat(&TokenKind::Semicolon).is_some() {
                fill = Some(self.child(base, &mut reach, Self::expr)?);
                break;
            }
            if self.eat(&TokenKind::Comma).is_none() {
                break;
            }
        }
        let close = self.expect(&TokenKind::RBracket)?;
        self.depth = reach;
        let kind = match (fill, elements.pop()) {
            (Some(len), Some(value)) => ExprKind::ArrayFill {
                value: Box::new(value),
                len: Box::new(len),
            },
            (_, last) => {
                elements.extend(last);
                ExprKind::ArrayLit(elements)
            }
        };
        Ok(Expr {
            kind,
            span: open.to(close.span),
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per kind of operand, so the dispatch reads in one place"
    )]
    fn primary(&mut self) -> Result<Expr, Diagnostic> {
        let token = self.peek().clone();
        let kind = match token.kind {
            TokenKind::Int(value) => {
                // `MIN_MAGNITUDE` fits only as the operand of a `-`, which `unary` takes first.
                let value = i64::try_from(value).map_err(|_| literal_too_large(token.span))?;
                self.bump();
                ExprKind::Int {
                    value,
                    literal: token.span,
                }
            }
            TokenKind::Float(x) => {
                self.bump();
                ExprKind::Float(x)
            }
            // A `.` that opens an operand and touches a decimal integer is `.5`, which lacks its
            // leading zero. The item is broken, so nothing after the dot cascades.
            TokenKind::Dot if self.leading_dot_float() => {
                return Err(DiagnosticKind::FloatNeedsDigits {
                    at: token.span,
                    fix: Span::new(token.span.start(), token.span.start()),
                }
                .into());
            }
            TokenKind::True | TokenKind::False => {
                self.bump();
                ExprKind::Bool(token.kind == TokenKind::True)
            }
            TokenKind::Char(c) => {
                self.bump();
                ExprKind::Char(c)
            }
            TokenKind::Str(s) => {
                self.bump();
                ExprKind::Str(s)
            }
            TokenKind::StrHead(head) => {
                self.bump();
                return self.interpolation(token.span, head);
            }
            TokenKind::SelfKw => {
                self.bump();
                ExprKind::SelfValue
            }
            TokenKind::NoneKw => {
                self.bump();
                ExprKind::NoneLit
            }
            TokenKind::Ident(_) if matches!(self.after_label(), Some((_, TokenKind::Loop))) => {
                let label = self.loop_label();
                return self.loop_expr(label);
            }
            TokenKind::Ident(name) => {
                self.bump();
                if let Some(error) = self.path_idiom(token.span) {
                    return Err(error);
                }
                if self.at(&TokenKind::LParen) {
                    let (args, close) = self.args()?;
                    return Ok(Expr {
                        span: token.span.to(close),
                        kind: ExprKind::Call {
                            callee: Ident {
                                name,
                                span: token.span,
                            },
                            args,
                        },
                    });
                }
                ExprKind::Name(name)
            }
            TokenKind::Todo => return self.todo(),
            TokenKind::LParen => {
                self.bump();
                let inner = self.expr()?;
                let close = self.expect(&TokenKind::RParen)?;
                // The parentheses are part of the span, so diagnostics underline them.
                return Ok(Expr {
                    span: token.span.to(close.span),
                    ..inner
                });
            }
            TokenKind::LBracket => {
                self.bump();
                return self.array(token.span);
            }
            TokenKind::If => return self.if_expr(),
            TokenKind::Loop => return self.loop_expr(None),
            TokenKind::Match => return self.match_expr(),
            TokenKind::LBrace => {
                let block = self.block()?;
                return Ok(Expr {
                    span: block.span,
                    kind: ExprKind::Block(block),
                });
            }
            TokenKind::Underscore => {
                return Err(DiagnosticKind::UnderscoreValue {
                    at: token.span,
                    bind: false,
                }
                .into());
            }
            TokenKind::Pipe => return Err(self.lone_pipe()),
            _ => return Err(self.expected("an expression")),
        };
        Ok(Expr {
            kind,
            span: token.span,
        })
    }

    /// Whether the `.` here is followed, with no gap, by an integer written in decimal.
    fn leading_dot_float(&self) -> bool {
        // `Dot` is never `Eof`, so the token after it exists.
        let next = &self.tokens[self.pos + 1];
        matches!(next.kind, TokenKind::Int(_))
            && next.span.start() == self.peek().span.end()
            && self.source[next.span.range()]
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'_')
    }

    /// `todo` or `todo(args)`. Rust's `todo!` and `todo!(...)`, with the `!` on the keyword's
    /// line, and an empty `todo()`, are reported with the fix that deletes what is extra.
    fn todo(&mut self) -> Result<Expr, Diagnostic> {
        let keyword = self.bump().span;
        let (tokens, pos) = (self.tokens, self.pos);
        let next = |n: usize| &tokens[(pos + n).min(tokens.len() - 1)];
        // Only a `!` on the keyword's line is Rust's `todo!`: inside brackets, where no `Newline`
        // token ends the line, one on a later line is the next token's business.
        // `breaks` is sorted, so the lookup is a binary search, and only a `!` pays for it.
        let same_line = matches!(next(0).kind, TokenKind::Bang) && {
            let at = self.breaks.partition_point(|&b| b < keyword.end());
            self.breaks
                .get(at)
                .is_none_or(|&b| b >= next(0).span.start())
        };
        let (at, delete) = match (&next(0).kind, &next(1).kind, &next(2).kind) {
            (TokenKind::Bang, TokenKind::LParen, TokenKind::RParen) if same_line => {
                (next(0).span, next(0).span.to(next(2).span))
            }
            (TokenKind::Bang, ..) if same_line => (next(0).span, next(0).span),
            (TokenKind::LParen, TokenKind::RParen, _) => {
                (next(0).span, next(0).span.to(next(1).span))
            }
            (TokenKind::LParen, ..) => {
                let (args, close) = self.args()?;
                return Ok(Expr {
                    kind: ExprKind::Todo { keyword, args },
                    span: keyword.to(close),
                });
            }
            _ => {
                return Ok(Expr {
                    kind: ExprKind::Todo {
                        keyword,
                        args: Vec::new(),
                    },
                    span: keyword,
                });
            }
        };
        // A word right after the deleted text would join `todo`: `todo!x` is not `todo x`.
        let rest = &self.source[delete.end()..];
        let safe = !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_');
        Err(DiagnosticKind::TodoForm { at, delete, safe }.into())
    }

    /// The rest of a string literal with interpolations, after its `StrHead` at `start` with the
    /// text `head`: each expression, and the text after it, up to the `StrTail`.
    fn interpolation(&mut self, start: Span, head: String) -> Result<Expr, Diagnostic> {
        let text = |value: String, span| {
            (!value.is_empty()).then_some(Part {
                value: Expr {
                    kind: ExprKind::Str(value),
                    span,
                },
                spec: None,
            })
        };
        let base = self.depth;
        let mut reach = base;
        let mut parts: Vec<Part> = text(head, start).into_iter().collect();
        let end = loop {
            let value = self.child(base, &mut reach, Self::expr)?;
            let mut spec = None;
            if let TokenKind::FormatSpec(written) = &self.peek().kind {
                let at = self.bump().span;
                spec = format_spec(written, at)
                    .map_err(|error| self.specs.push(error))
                    .ok();
            }
            parts.push(Part { value, spec });
            let token = self.peek().clone();
            match token.kind {
                TokenKind::StrMiddle(value) => parts.extend(text(value, token.span)),
                TokenKind::StrTail(value) => {
                    parts.extend(text(value, token.span));
                    self.bump();
                    break token.span;
                }
                _ => return Err(self.expected("`}`")),
            }
            self.bump();
        };
        self.depth = reach;
        Ok(Expr {
            kind: ExprKind::Interpolate(parts),
            span: start.to(end),
        })
    }

    /// `(` arguments `)`, and the span of the `)`.
    fn args(&mut self) -> Result<(Vec<Arg>, Span), Diagnostic> {
        self.expect(&TokenKind::LParen)?;
        let base = self.depth;
        let mut reach = base;
        let mut args = Vec::new();
        if !self.at(&TokenKind::RParen) {
            loop {
                args.push(self.child(base, &mut reach, Self::arg)?);
                if self.eat(&TokenKind::Comma).is_none() {
                    break;
                }
            }
        }
        let close = self.expect(&TokenKind::RParen)?;
        self.depth = reach;
        Ok((args, close.span))
    }

    /// A label is an `Ident` followed by `:`, so it takes two tokens of lookahead.
    fn arg(&mut self) -> Result<Arg, Diagnostic> {
        let labeled = matches!(self.peek().kind, TokenKind::Ident(_))
            && self
                .tokens
                .get(self.pos + 1)
                .is_some_and(|t| t.kind == TokenKind::Colon);
        let label = if labeled {
            let label = self.ident("a label")?;
            self.bump();
            if let Some(error) = self.label_not_on_loop() {
                return Err(error);
            }
            Some(label)
        } else {
            None
        };
        Ok(Arg {
            label,
            value: self.expr()?,
        })
    }

    fn if_expr(&mut self) -> Result<Expr, Diagnostic> {
        let start = self.expect(&TokenKind::If)?.span;
        let base = self.depth;
        let mut reach = base;
        let head = match self.child(base, &mut reach, Self::let_binding)? {
            Some((pattern, value)) => Head::Let(pattern, value),
            None => Head::Cond(self.child(base, &mut reach, Self::expr)?),
        };
        let then_block = self.child(base, &mut reach, Self::block)?;
        let mut end = then_block.span;
        let else_block = if self.eat(&TokenKind::Else).is_some() {
            let block = if self.at(&TokenKind::If) {
                // `else if` and `else if let` become a block holding the inner `if`.
                let inner = self.child(base, &mut reach, |p| {
                    p.descend()?;
                    p.if_expr()
                })?;
                Block {
                    span: inner.span,
                    stmts: vec![Stmt {
                        span: inner.span,
                        kind: StmtKind::Expr(inner),
                    }],
                }
            } else {
                self.child(base, &mut reach, Self::block)?
            };
            end = block.span;
            Some(block)
        } else {
            None
        };
        let kind = match head {
            Head::Cond(cond) => ExprKind::If {
                cond: Box::new(cond),
                then_block,
                else_block,
            },
            Head::Let(pattern, value) => ExprKind::IfLet {
                pattern,
                value: Box::new(value),
                then_block,
                else_block,
            },
        };
        self.depth = reach;
        Ok(Expr {
            span: start.to(end),
            kind,
        })
    }

    /// `loop { body }`, after its loop label when it has one.
    fn loop_expr(&mut self, label: Option<Ident>) -> Result<Expr, Diagnostic> {
        let keyword = self.expect(&TokenKind::Loop)?.span;
        let base = self.depth;
        let mut reach = base;
        let body = self.child(base, &mut reach, Self::block);
        // The loop label closes with the body, also when the body fails to parse.
        if label.is_some() {
            self.labels.pop();
        }
        let body = body?;
        self.depth = reach;
        let start = label.as_ref().map_or(keyword, |l| l.span);
        Ok(Expr {
            span: start.to(body.span),
            kind: ExprKind::Loop {
                label,
                keyword,
                body,
            },
        })
    }
}

/// A name, a field of any expression, or an index of a place.
fn is_place(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Name(_) | ExprKind::Field { .. } => true,
        ExprKind::Index { base, .. } => is_place(base),
        _ => false,
    }
}

/// Whether a `?.` sits anywhere in the chain of fields, calls and indexes that makes up `e`.
fn goes_through_safe_access(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Field { base, safe, .. } => *safe || goes_through_safe_access(base),
        ExprKind::MethodCall { receiver, safe, .. } => *safe || goes_through_safe_access(receiver),
        ExprKind::Index { base, .. } => goes_through_safe_access(base),
        _ => false,
    }
}

/// What follows `if`: a condition, or the `let pattern = value` of an `if let`.
enum Head {
    Cond(Expr),
    Let(Pattern, Expr),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Code;
    use crate::diagnostic::explanations::{apply_fix, fix_edits};
    use crate::lexer::lex;
    use crate::source::line_col;

    /// Ranges may overlap, as an unnamed item does the item before it, but neither starts nor ends
    /// may go back.
    #[test]
    fn rising_ranges() {
        assert!(rising(&[0..2, 1..9, 9..9]));
        assert!(!rising(&[0..5, 1..4]));
        assert!(!rising(&[1..5, 0..6]));
        assert!(holds(&[0..2, 1..9], 1) && holds(&[0..2, 1..9], 8) && !holds(&[0..2, 3..9], 2));
    }

    fn program(src: &str) -> String {
        parse_ok(src).to_string()
    }

    /// The first item, which must be a function.
    fn first_fn(src: &str) -> Function {
        match parse_ok(src).items.into_iter().next() {
            Some(Item::Function(f)) => f,
            other => panic!("expected a function, found {other:?}"),
        }
    }

    fn body(src: &str) -> String {
        first_fn(src).body.to_string()
    }

    /// The parse of `src`, which must lex without a bracket error.
    fn parsed(src: &str) -> Parsed {
        parse(lex(src).expect("source lexes"))
    }

    /// The one diagnostic `src` reports.
    fn parse_diagnostic(src: &str) -> Diagnostic {
        let parsed = parsed(src);
        let [d] = parsed.diagnostics.as_slice() else {
            panic!(
                "{src}: expected one diagnostic, got {:?}",
                parsed.diagnostics
            );
        };
        d.clone()
    }

    fn parse_err(src: &str) -> (Code, usize, usize) {
        let d = parse_diagnostic(src);
        let (line, col) = line_col(src, d.span().start());
        (d.code(), line, col)
    }

    /// `src` with the one fix of its parse error applied, which must be safe.
    fn fixed(src: &str) -> String {
        let d = parse_diagnostic(src);
        let (safe, _) = fix_edits(&d).expect("the error has a fix");
        assert!(safe, "{src}");
        apply_fix(src, &d)
    }

    #[test]
    fn function_with_let() {
        assert_eq!(
            program("fn main() {\n    let x = 1\n}"),
            "(fn main () unit (block (let x 1)))"
        );
    }

    #[test]
    fn params_and_return_type() {
        assert_eq!(
            program("fn f(a: i64, b: bool) -> i64 { a }"),
            "(fn f ((a i64) (b bool)) i64 (block a))"
        );
    }

    /// Each "expected X" error at the token found instead. The first four name a place in the
    /// grammar and the last three the token itself; the `E0011-expected-*` fixtures pin each text.
    #[test]
    fn expectation_errors_sit_on_the_token_found() {
        for (src, line, col) in [
            ("class {}", 1, 7),
            ("class A {\n    mut 1: i64\n}", 2, 9),
            ("fn f(a: ) {}", 1, 9),
            ("fn main() {\n    print([1].0)\n}", 2, 15),
            ("fn f(a) {}", 1, 7),
            ("fn main() {\n    for i 0..3 {\n    }\n}", 2, 11),
            ("fn f {}", 1, 6),
        ] {
            assert_eq!(parse_err(src), (Code::Expected, line, col), "{src}");
        }
    }

    #[test]
    fn float_literal_is_an_expression() {
        assert_eq!(
            body("fn main() { 1.5e3 + 0.25 }"),
            "(block (+ 1500.0 0.25))"
        );
        assert_eq!(body("fn main() { -2.5 }"), "(block (- 2.5))");
    }

    /// A `.` that opens an operand and is followed at once by a decimal integer is the missing
    /// leading zero of a float, wherever an operand stands.
    #[test]
    fn leading_dot_float_is_reported_at_the_dot() {
        for (src, line, col) in [
            ("fn main() {\n    let x = .5\n}", 2, 13),
            ("fn main() {\n    f(.5)\n}", 2, 7),
            ("fn main() {\n    let x = 1 + .5\n}", 2, 17),
            ("fn main() {\n    let x = -.5\n}", 2, 14),
            ("fn main() {\n    let x = .1_0\n}", 2, 13),
        ] {
            assert_eq!(parse_err(src), (Code::FloatNeedsDigits, line, col), "{src}");
        }
    }

    /// Digits after a field access's `.` are not a name, however many there are.
    #[test]
    fn digits_after_a_field_dot_are_not_a_name() {
        for src in [
            "fn main() {\n    x.5\n}",
            "fn main() {\n    x.99999999999999999999\n}",
        ] {
            assert_eq!(parse_err(src), (Code::Expected, 2, 7), "{src}");
        }
    }

    /// Anything else after a leading `.` is an expression that is missing.
    #[test]
    fn leading_dot_without_a_decimal_integer_is_not_a_float() {
        for src in [
            "fn main() {\n    let x = . 5\n}",
            "fn main() {\n    let x = .0x5\n}",
            "fn main() {\n    let x = .x\n}",
        ] {
            assert_eq!(parse_err(src), (Code::Expected, 2, 13), "{src}");
        }
    }

    /// Each float error has one safe fix that leaves a program that parses; the lexer finds
    /// `1.` and the parser `.5`.
    #[test]
    fn float_fixes() {
        for (src, fixed_src) in [
            ("let x = 1.", "let x = 1.0"),
            ("let x = .5", "let x = 0.5"),
            ("let x = .5e3", "let x = 0.5e3"),
        ] {
            let wrap = |s: &str| format!("fn main() {{\n    {s}\n}}");
            assert_eq!(parse_diagnostic(&wrap(src)).code(), Code::FloatNeedsDigits);
            let result = fixed(&wrap(src));
            assert_eq!(result, wrap(fixed_src));
            parse_ok(&result);
        }
    }

    /// A `char` literal where an expression ends is the unexpected token.
    #[test]
    fn char_is_an_unexpected_token() {
        assert_eq!(
            parse_err("fn main() {\n    let b = 'x' 'y'\n}"),
            (Code::Expected, 2, 17)
        );
    }

    /// A `-` applied directly to 2^63 is the one literal `i64::MIN`, with the `-` in its span.
    #[test]
    fn minus_min_magnitude_is_one_literal() {
        assert_eq!(
            body("fn main() { -9223372036854775808 }"),
            "(block -9223372036854775808)"
        );
        assert_eq!(
            body("fn main() { -   9_223_372_036_854_775_808 }"),
            "(block -9223372036854775808)"
        );
        let f = first_fn("fn main() { 1 - -9223372036854775808 }");
        let StmtKind::Expr(sum) = &f.body.stmts[0].kind else {
            panic!("expected an expression statement")
        };
        assert_eq!(sum.to_string(), "(- 1 -9223372036854775808)");
        assert_eq!((sum.span.start(), sum.span.end()), (12, 36));
    }

    /// The magnitude of `i64::MIN` is `i64::MIN` after a `-` in any base, and too large without.
    #[test]
    fn min_magnitude_in_hex() {
        assert_eq!(
            body("fn main() { -0x8000_0000_0000_0000 }"),
            "(block -9223372036854775808)"
        );
        assert_eq!(
            parse_err("fn main() { 0x8000_0000_0000_0000 }"),
            (Code::IntegerTooLarge, 1, 13)
        );
    }

    /// Anywhere else 2^63 is the one "too large" error, at the literal. The lexer rejects larger.
    #[test]
    fn min_magnitude_elsewhere_is_too_large() {
        for (src, col) in [
            ("fn main() { 9223372036854775808 }", 13),
            ("fn main() { -(9223372036854775808) }", 15),
            ("fn main() { 1 - 9223372036854775808 }", 17),
            // The `.` and `[` bind tighter than the `-`, so they apply to the literal.
            ("fn main() { -9223372036854775808.x }", 14),
            ("fn main() { -9223372036854775808?.x }", 14),
            ("fn main() { -9223372036854775808[0] }", 14),
        ] {
            assert_eq!(parse_err(src), (Code::IntegerTooLarge, 1, col), "{src}");
        }
    }

    #[test]
    fn interpolation_s_expression() {
        assert_eq!(
            expr(r#"print("a{x}b{ "c" + y }")"#),
            r#"(call print (interp "a" x "b" (+ "c" y)))"#
        );
        // Empty text pieces are dropped.
        assert_eq!(expr(r#"print("{x}{y}")"#), "(call print (interp x y))");
    }

    /// A part with a spec prints as `fmt`, its spec in the order the grammar gives.
    #[test]
    fn format_spec_s_expression() {
        assert_eq!(
            expr(r#"print("a{x:>8.2}b{y}{z:.1}{w:4}")"#),
            r#"(call print (interp "a" (fmt x ">8.2") "b" y (fmt z ".1") (fmt w "4")))"#
        );
    }

    #[test]
    fn format_specs() {
        use crate::ast::Align::{Center, Left, Right};
        let at = Span::new(0, 0);
        for (text, align, width, precision) in [
            (">8.2", Some(Right), Some(8), Some(2)),
            ("<10", Some(Left), Some(10), None),
            ("^5", Some(Center), Some(5), None),
            (".3", None, None, Some(3)),
            ("7", None, Some(7), None),
            ("1000.30", None, Some(1000), Some(30)),
            ("<.0", Some(Left), None, Some(0)),
        ] {
            let spec = format_spec(text, at).expect(text);
            assert_eq!(
                (spec.align, spec.width, spec.precision),
                (align, width, precision),
                "{text}"
            );
        }
        // Each error points at the whole spec, its `:` included.
        let (malformed, unsupported, limit) = (
            Code::MalformedFormatSpec,
            Code::UnsupportedFormatSpec,
            Code::FormatSpecLimit,
        );
        for (spec, code) in [
            ("", malformed),
            (">", malformed),
            (".", malformed),
            ("8.", malformed),
            ("^.", malformed),
            ("03", unsupported),
            ("0", unsupported),
            ("+", unsupported),
            ("e", unsupported),
            ("x", unsupported),
            (".2e", unsupported),
            ("*>5", unsupported),
            (" 5", unsupported),
            (">{w}", unsupported),
            ("1001", limit),
            ("65536", limit),
            ("99999999999999999999", limit),
            (".31", limit),
            (".256", limit),
            ("5.99999999999999999999", limit),
        ] {
            let src = format!("fn main() {{\n    print(\"{{x:{spec}}}\")\n}}");
            let d = parse_diagnostic(&src);
            let (line, col) = line_col(&src, d.span().start());
            assert_eq!((d.code(), line, col), (code, 2, 14), "{spec}");
            assert_eq!(&src[d.span().range()], format!(":{spec}"), "{spec}");
        }
    }

    /// An interpolation holds one expression, which ends at its `}`.
    #[test]
    fn interpolation_errors() {
        for (src, col) in [(r#""{x y}""#, 17), (r#""{x +}""#, 18)] {
            assert_eq!(expr_err(src), (Code::Expected, 1, col), "{src}");
        }
    }

    #[test]
    fn char_s_expression() {
        assert_eq!(
            body("fn main() {\n    let c = 'ä'\n    let n = '\\n'\n}"),
            "(block (let c 'ä') (let n '\\n'))"
        );
    }

    #[test]
    fn typed_let_and_let_mut() {
        assert_eq!(
            body("fn main() {\n    let x: i64 = 1\n    let mut y = 2\n}"),
            "(block (let x i64 1) (let mut y 2))"
        );
    }

    #[test]
    fn function_span() {
        assert_eq!(first_fn("fn main() {\n}\n").span, Span::new(0, 13));
    }

    #[test]
    fn return_without_value_before_brace() {
        assert_eq!(body("fn main() { return }"), "(block (return))");
    }

    #[test]
    fn return_with_value() {
        assert_eq!(
            body("fn f() -> i64 {\n    return 1\n}"),
            "(block (return 1))"
        );
    }

    #[test]
    fn break_and_continue() {
        assert_eq!(
            body("fn main() {\n    while c {\n        break\n        continue\n    }\n}"),
            "(block (while c (block (break) (continue))))"
        );
    }

    #[test]
    fn loop_is_an_expression() {
        assert_eq!(expr("loop { }"), "(loop (block))");
        assert_eq!(expr("let x = loop { }"), "(let x (loop (block)))");
    }

    /// `break` takes a value only when the next token can start one.
    #[test]
    fn break_value_ends_where_a_jump_ends() {
        for (src, printed) in [
            ("break 1", "(break 1)"),
            ("break -1", "(break (- 1))"),
            ("break\n f()", "(break)"),
            ("break; f()", "(break)"),
            ("break", "(break)"),
        ] {
            assert_eq!(expr(src), printed, "{src}");
        }
        // `,` ends the jump too, so the arm list, not a value, reports it; `E0011-break-arrow`
        // pins `=>`.
        assert_eq!(
            expr_err("match o { 1 => break, _ => x }"),
            (Code::FieldComma, 1, 33)
        );
        // Anything else starts a value, and a value that does not parse is the error.
        assert_eq!(expr_err("break 1 +"), (Code::Expected, 1, 23));
        // No `)` or `]` rows: one right after `break` would close a bracket opened outside the
        // enclosing `{`, which the lexer rejects before parsing.
    }

    #[test]
    fn continue_takes_no_value() {
        assert_eq!(expr_err("continue 1"), (Code::Expected, 1, 22));
    }

    /// A loop label prints after its loop's keyword, on each kind of loop and as a value.
    #[test]
    fn loop_labels() {
        for (src, printed) in [
            ("outer: for x in xs { }", "(for :outer x xs (block))"),
            ("outer: while c { }", "(while :outer c (block))"),
            (
                "outer: while let y = o { }",
                "(while-let :outer y o (block))",
            ),
            ("rows: loop { }", "(loop :rows (block))"),
            ("let v = rows: loop { }", "(let v (loop :rows (block)))"),
            // The argument label comes first, so the second name labels the loop.
            (
                "Box(outer: rows: loop { })",
                "(call Box (: outer (loop :rows (block))))",
            ),
            (
                "match x { x => outer: loop { break outer } }",
                "(match x (arm x (loop :outer (block (break :outer)))))",
            ),
        ] {
            assert_eq!(expr(src), printed, "{src}");
        }
        // The lexer joins the line after `:`, so the label and its loop are one statement.
        assert_eq!(
            body("fn main() {\n    outer:\n    for x in xs { }\n}"),
            "(block (for :outer x xs (block)))"
        );
        // Before anything but `loop`, `while` or `for`, `name:` is no label, and the `:` is the
        // error it was before labels.
        assert_eq!(
            parse_err("fn main() {\n    x: i64 = 1\n}"),
            (Code::Expected, 2, 6)
        );
    }

    /// `name:` before `if`, `match` or `{` is E0145 wherever a labeled loop could stand: a
    /// statement, the value of a `let` or an assignment, and an argument after its label. `_:`
    /// before a loop is E0145 too, and its safe fix deletes it.
    #[test]
    fn labels_not_on_loops() {
        for (src, at) in [
            (
                "fn main() {\n    x = outer: if c { 1 } else { 2 }\n}",
                (2, 9),
            ),
            (
                "fn main() {\n    x += outer: match c { _ => 1 }\n}",
                (2, 10),
            ),
            ("fn main() {\n    f(n: outer: { 1 })\n}", (2, 10)),
        ] {
            assert_eq!(parse_err(src), (Code::LabelNotOnLoop, at.0, at.1), "{src}");
        }
        assert_eq!(
            fixed("fn main() {\n    _: while c { }\n}"),
            "fn main() {\n    while c { }\n}"
        );
        assert_eq!(
            parse_err("fn main() {\n    _: loop { }\n}"),
            (Code::LabelNotOnLoop, 2, 5)
        );
        // A `=` with no value after it is the missing value, not a loop label.
        assert_eq!(
            parse_err("fn main() {\n    let x =\n}"),
            (Code::Expected, 3, 1)
        );
    }

    /// After `break`, a name is the loop label only while a loop with that label is open around
    /// the jump, its own header included; after `continue`, any name is.
    #[test]
    fn jump_labels() {
        for (src, printed) in [
            (
                "outer: for x in xs { break outer -1 }",
                "(for :outer x xs (block (break :outer (- 1))))",
            ),
            (
                "for x in xs { break outer -1 }",
                "(for x xs (block (break (- outer 1))))",
            ),
            (
                "outer: for x in xs { continue outer }",
                "(for :outer x xs (block (continue :outer)))",
            ),
            (
                "for x in xs { continue outer }",
                "(for x xs (block (continue :outer)))",
            ),
            (
                "outer: while ok(loop { break outer - 1 }) { }",
                "(while :outer (call ok (loop (block (break :outer (- 1))))) (block))",
            ),
        ] {
            assert_eq!(expr(src), printed, "{src}");
        }
        // A label closes with its loop's body, so a sibling may take it again, and a jump after
        // both reads the name as a value.
        assert_eq!(
            body(
                "fn main() {\n    outer: loop { break outer }\n    outer: loop { break outer }\n    loop { break outer }\n}"
            ),
            "(block (loop :outer (block (break :outer))) (loop :outer (block (break :outer))) (loop (block (break outer))))"
        );
        // A `while` or `for` label, which a statement opens, closes after its body too.
        assert_eq!(
            body("fn main() {\n    outer: for x in xs { }\n    loop { break outer -1 }\n}"),
            "(block (for :outer x xs (block)) (loop (block (break (- outer 1)))))"
        );
    }

    #[test]
    fn while_loop() {
        assert_eq!(
            body("fn main() {\n    while c { f() }\n}"),
            "(block (while c (block (call f))))"
        );
    }

    #[test]
    fn else_if_chain() {
        assert_eq!(
            body("fn main() { if a { 1 } else if b { 2 } else { 3 } }"),
            "(block (if a (block 1) (block (if b (block 2) (block 3)))))"
        );
    }

    #[test]
    fn multiline_call_args() {
        assert_eq!(
            body("fn main() {\n    f(1,\n      2)\n}"),
            "(block (call f 1 2))"
        );
    }

    #[test]
    fn else_on_new_line_fix() {
        // After an `if` or `if let` without `else`, the gap after the `}` becomes one space.
        for (src, position, gap) in [
            (
                "fn main() {\n    if a {\n    }\n    else {\n    }\n}",
                (4, 5),
                (28, 33),
            ),
            (
                "fn main() {\n    if let n = x {\n    }\n    else {\n    }\n}",
                (4, 5),
                (36, 41),
            ),
            (
                "fn main() {\n    if a {}\n    else if b {}\n}",
                (3, 5),
                (23, 28),
            ),
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::ElseOnNewLine, "{src}");
            assert_eq!(line_col(src, d.span().start()), position, "{src}");
            assert_eq!(
                fix_edits(&d),
                Some((true, vec![(gap.0, gap.1, " ".to_string())])),
                "{src}"
            );
        }
    }

    #[test]
    fn safe_else_fix_removes_the_error() {
        let src = "fn main() {\n    if let n = x {\n        print(n)\n    }\n    else if b {\n        print(2)\n    }\n}\n";
        let result = fixed(src);
        assert_eq!(
            result,
            "fn main() {\n    if let n = x {\n        print(n)\n    } else if b {\n        print(2)\n    }\n}\n"
        );
        parse_ok(&result);
    }

    #[test]
    fn else_on_new_line_is_unsafe_over_a_comment() {
        // The join would delete the comment.
        let src = "fn main() {\n    if a {\n    } // done\n    else {\n    }\n}";
        let d = parse_diagnostic(src);
        assert_eq!(
            (d.code(), line_col(src, d.span().start())),
            (Code::ElseOnNewLine, (4, 5))
        );
        assert!(fix_edits(&d).is_some_and(|(safe, _)| !safe));
    }

    #[test]
    fn else_on_new_line_has_no_fix_after_anything_else() {
        for src in [
            // A loop's `}` cannot take an `else`.
            "fn main() {\n    while a {\n    }\n    else {\n    }\n}",
            // The `if` already has its `else`.
            "fn main() {\n    if a {\n    } else {\n    }\n    else {\n    }\n}",
            "fn main() {\n    if a {\n    } else if b {\n    }\n    else {\n    }\n}",
            // Nothing before it.
            "fn main() {\n    else {\n    }\n}",
            "fn main() {\n    print(1)\n    else {\n    }\n}",
            // The `else` has no block to open.
            "fn main() {\n    if a {\n    }\n    else b\n}",
            // Only an `if` statement can take it: not a `let`, nor the `if` inside one.
            "fn main() {\n    let x = 1\n    else {\n    }\n}",
            "fn main() {\n    let x = if a { 1 }\n    else { 2 }\n}",
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::ElseOnNewLine, "{src}");
            assert_eq!(fix_edits(&d), None, "{src}");
        }
    }

    #[test]
    fn two_statements_on_one_line() {
        assert_eq!(
            parse_err("fn main() { let x = 1 2 }"),
            (Code::Expected, 1, 23)
        );
    }

    #[test]
    fn unexpected_eof() {
        let src = "fn\n";
        assert_eq!(parse_err(src), (Code::Expected, 1, 3));
    }

    #[test]
    fn top_level_statement() {
        assert_eq!(parse_err("let x = 1"), (Code::Expected, 1, 1));
    }

    /// Parses `fn main() { <src> }` and prints the first statement.
    fn expr(src: &str) -> String {
        first_fn(&format!("fn main() {{ {src} }}")).body.stmts[0].to_string()
    }

    /// Error code, line and column for `fn main() { <src> }`; the columns start at 13.
    fn expr_err(src: &str) -> (Code, usize, usize) {
        parse_err(&format!("fn main() {{ {src} }}"))
    }

    #[test]
    fn new_tokens_are_errors_where_they_do_not_belong() {
        // `self` is an expression now, so its row hits a different error.
        for src in [
            "class",
            "is",
            "let self = 1",
            "let None = 1",
            ".",
            "?",
            "?.",
            "??",
        ] {
            assert_eq!(expr_err(src).0, Code::Expected, "{src}");
        }
    }

    #[test]
    fn expression_shapes() {
        let rows = [
            ("1 + 2 * 3", "(+ 1 (* 2 3))"),
            ("1 - 2 - 3", "(- (- 1 2) 3)"),
            ("-a * b", "(* (- a) b)"),
            ("!a && b || c", "(|| (&& (! a) b) c)"),
            ("(1 + 2) * 3", "(* (+ 1 2) 3)"),
            ("f(1, g(2))", "(call f 1 (call g 2))"),
            ("a < b", "(< a b)"),
            ("a < b <= c", "(< a b <= c)"),
            ("a > b >= c", "(> a b >= c)"),
            ("a == b < c", "(== a (< b c))"),
            ("a != b", "(!= a b)"),
            ("(a < b) < c", "(< (< a b) c)"),
            ("true && false", "(&& true false)"),
            ("x += 1", "(+= x 1)"),
            ("x %= 2", "(%= x 2)"),
            ("x -= 1", "(-= x 1)"),
            ("x *= 2", "(*= x 2)"),
            ("x /= 2", "(/= x 2)"),
            ("(x) = 1", "(= x 1)"),
            ("a.b.c", "(. (. a b) c)"),
            ("t.insert(1, 2)", "(call (. t insert) 1 2)"),
            (
                "Tree(value: 1, left: x)",
                "(call Tree (: value 1) (: left x))",
            ),
            ("-a.b", "(- (. a b))"),
            ("E()", "(call E)"),
            ("a[i].f = v", "(= (. (index a i) f) v)"),
            ("make().count = 1", "(= (. (call make) count) 1)"),
            ("a.b[i].c[j] = v", "(= (index (. (index (. a b) i) c) j) v)"),
            (
                "make().cells[0] = 1",
                "(= (index (. (call make) cells) 0) 1)",
            ),
            ("a.f += 1", "(+= (. a f) 1)"),
            ("self.left = t", "(= (. self left) t)"),
            ("None", "None"),
            ("f(None)", "(call f None)"),
            ("x ?? 0 < 5", "(< (?? x 0) 5)"),
            ("x ?? 0 + 1", "(?? x (+ 0 1))"),
            ("a ?? b ?? 0", "(?? a (?? b 0))"),
            ("x ?? 0 == 1", "(== (?? x 0) 1)"),
            ("a?.b?.c()", "(call (?. (?. a b) c))"),
            ("a?.b.c", "(. (?. a b) c)"),
            ("!(a is b)", "(! (is a b))"),
            ("a == b && c is d", "(&& (== a b) (is c d))"),
        ];
        for (src, expected) in rows {
            assert_eq!(expr(src), expected, "{src}");
        }
    }

    #[test]
    fn parenthesized_primary() {
        assert_eq!(body("fn main() { (f()) }"), "(block (call f))");
    }

    #[test]
    fn block_expression_positions() {
        let rows = [
            ("let a = { 1 }", "(let a (block 1))"),
            ("f({ 1 })", "(call f (block 1))"),
            ("1 + { 2 }", "(+ 1 (block 2))"),
            ("if { c } { }", "(if (block c) (block))"),
            ("while { c } { }", "(while (block c) (block))"),
            ("for x in { xs } { }", "(for x (block xs) (block))"),
        ];
        for (src, expected) in rows {
            assert_eq!(expr(src), expected, "{src}");
        }
    }

    #[test]
    fn block_statement() {
        for (src, line, col) in [
            ("fn main() {\n    { 1 }\n}", 2, 5),
            ("fn main() {\n    if c { {1} }\n}", 2, 12),
        ] {
            assert_eq!(parse_err(src), (Code::BlockAsStatement, line, col), "{src}");
        }
    }

    #[test]
    fn compound_assignment_spans() {
        let tree = first_fn("fn main() { x += 1 }");
        let stmt = &tree.body.stmts[0];
        assert_eq!(stmt.span, Span::new(12, 18));
        assert!(
            matches!(stmt.kind, StmtKind::CompoundAssign { .. }),
            "{stmt}"
        );
    }

    #[test]
    fn opposite_comparison_directions() {
        assert_eq!(expr_err("a < b > c"), (Code::ComparisonDirection, 1, 19));
    }

    #[test]
    fn mixed_chain_directions_report_at_the_second_operator() {
        assert_eq!(expr_err("a > b <= c"), (Code::ComparisonDirection, 1, 19));
    }

    #[test]
    fn mixed_equality_operators_do_not_chain() {
        assert_eq!(expr_err("a == b != c"), (Code::EqualityChain, 1, 20));
    }

    #[test]
    fn equality_does_not_chain() {
        assert_eq!(
            parse_err("fn main() { a == b == c }"),
            (Code::EqualityChain, 1, 20)
        );
    }

    #[test]
    fn is_does_not_chain() {
        assert_eq!(
            parse_err("fn main() { print(a is b is c) }"),
            (Code::EqualityChain, 1, 26)
        );
    }

    #[test]
    fn mixed_is_and_equality_do_not_chain() {
        assert_eq!(expr_err("a is b == c"), (Code::EqualityChain, 1, 20));
        assert_eq!(expr_err("a == b is c"), (Code::EqualityChain, 1, 20));
    }

    #[test]
    fn cannot_assign_through_safe_access() {
        for src in [
            "a?.b = 1",
            "a?.b.c = 1",
            "a?.b += 1",
            "a?.m().c = 1",
            "a?.b.m().c = 1",
            "xs?.items[0].f = 1",
            "xs?.items[0].f += 1",
            "xs?.a[0] = 1",
        ] {
            let full = format!("fn main() {{ {src} }}");
            assert_eq!(
                parse_err(&full),
                (Code::AssignThroughSafeAccess, 1, 13),
                "{src}"
            );
        }
    }

    #[test]
    fn cannot_assign_to_self() {
        for src in ["self = 1", "self += 1"] {
            let full = format!("fn main() {{ {src} }}");
            assert_eq!(parse_err(&full), (Code::AssignToSelf, 1, 13), "{src}");
            let span = parse_diagnostic(&full).span();
            assert_eq!(span.end() - span.start(), 4, "{src}");
        }
    }

    #[test]
    fn cannot_assign_to_expression() {
        for src in ["1 + 2 = 3", "f()[0] = 1"] {
            assert_eq!(expr_err(src), (Code::AssignToExpression, 1, 13), "{src}");
        }
    }

    #[test]
    fn underscore_errors() {
        for (src, code, line, col) in [
            (
                "fn main() {\n    let mut _ = 1\n}",
                Code::MutOnUnderscore,
                2,
                13,
            ),
            ("fn main() {\n    print(_)\n}", Code::UnderscoreValue, 2, 11),
            ("fn main() {\n    _ = f()\n}", Code::UnderscoreValue, 2, 5),
            ("fn main() {\n    _ += 1\n}", Code::UnderscoreValue, 2, 5),
            ("fn _() {}", Code::UnderscoreName, 1, 4),
            ("class _ {}", Code::UnderscoreName, 1, 7),
            ("class C {\n    _: i64\n}", Code::UnderscoreName, 2, 5),
            ("class C {\n    mut _: i64\n}", Code::UnderscoreName, 2, 9),
            ("fn f(_: i64) {}", Code::UnderscoreName, 1, 6),
            ("fn f(a: i64, _: i64) {}", Code::UnderscoreName, 1, 14),
        ] {
            assert_eq!(parse_err(src), (code, line, col), "{src}");
        }
    }

    #[test]
    fn const_items() {
        for (src, want) in [
            ("const A = 1", "(const A 1)"),
            ("const B: u8 = 240", "(const B u8 240)"),
            // A constant is one line, so a `}` in its value does not end it.
            (
                "const C = if true { 1 } else { 2 }",
                "(const C (if true (block 1) (block 2)))",
            ),
            // A name as the last token is no loop label: no `:` and loop keyword follow it.
            ("const D = C", "(const D C)"),
        ] {
            assert_eq!(program(src), want, "{src}");
        }
        for (src, code, line, col) in [
            ("fn f() { const C = 1 }", Code::ConstInFunction, 1, 10),
            (
                "fn f() {\n    const C: u8 = 1\n}",
                Code::ConstInFunction,
                2,
                5,
            ),
            ("const _ = 1", Code::UnderscoreName, 1, 7),
            ("const A = 1 2", Code::Expected, 1, 13),
            ("const A 1", Code::Expected, 1, 9),
            ("class C {\n    const A = 1\n}", Code::Expected, 2, 5),
            // A `const` statement that does not parse as a `let`, or does not end its line,
            // reports that error first, so the fix to `let` is offered only when it parses.
            ("fn f() { const C }", Code::Expected, 1, 18),
            ("fn f() { const C = 1 2 }", Code::Expected, 1, 22),
        ] {
            assert_eq!(parse_err(src), (code, line, col), "{src}");
        }
    }

    #[test]
    fn const_statement_fix_leaves_a_let_that_parses() {
        for (src, want) in [
            ("fn f() { const C = 1 }", "fn f() { let C = 1 }"),
            ("fn f() { const mut C = 1 }", "fn f() { let mut C = 1 }"),
            ("fn f() { const _: u8 = 1 }", "fn f() { let _: u8 = 1 }"),
        ] {
            let fixed = fixed(src);
            assert_eq!(fixed, want);
            assert!(parsed(&fixed).diagnostics.is_empty(), "{fixed}");
        }
    }

    #[test]
    fn underscore_binds_after_let_and_for() {
        for src in [
            "fn main() { let _ = 1 }",
            "fn main() { let _: i64 = 1 }",
            "fn main() { for _ in 0..3 {} }",
            "fn main() { for _ in xs {} }",
        ] {
            assert!(parsed(src).diagnostics.is_empty(), "{src}");
        }
    }

    #[test]
    fn mut_on_underscore_fix_leaves_a_let_that_parses() {
        for (src, want) in [
            ("fn main() { let mut _ = 1 }", "fn main() { let _ = 1 }"),
            (
                "fn main() { let mut _: i64 = 1 }",
                "fn main() { let _: i64 = 1 }",
            ),
        ] {
            let fixed = fixed(src);
            assert_eq!(fixed, want);
            assert!(parsed(&fixed).diagnostics.is_empty(), "{fixed}");
        }
        // A `let` that does not parse, or does not end its line, reports its own error first and
        // offers no fix: applying one would leave the same error behind.
        assert_eq!(
            parse_err("fn main() { let mut _ }"),
            (Code::Expected, 1, 23)
        );
        assert_eq!(
            parse_err("fn main() { let mut _ = 1 2 }"),
            (Code::Expected, 1, 27)
        );
        // The fixed source reports the same error, four columns (`mut `) earlier.
        assert_eq!(
            parse_err("fn main() { let _ = 1 2 }"),
            (Code::Expected, 1, 23)
        );
    }

    #[test]
    fn assigning_to_underscore_offers_let_only_when_the_value_parses() {
        for (src, want) in [
            ("fn main() { _ = f() }", "fn main() { let _ = f() }"),
            ("fn main() {\n    _ = 1\n}", "fn main() {\n    let _ = 1\n}"),
        ] {
            let fixed = fixed(src);
            assert_eq!(fixed, want);
            assert!(parsed(&fixed).diagnostics.is_empty(), "{fixed}");
        }
        for src in [
            "fn main() { _ += 1 }",
            "fn main() { _[0] = 1 }",
            "fn main() { _ == 1 }",
            "fn main() { _ = 1 2 }",
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::UnderscoreValue, "{src}");
            assert!(fix_edits(&d).is_none(), "{src}");
        }
        // The value's own error comes first.
        assert_eq!(parse_err("fn main() { _ = 1 + }"), (Code::Expected, 1, 21));
    }

    #[test]
    fn missing_initializer_at_end_of_line() {
        assert_eq!(
            parse_err("fn main() {\n    let x = 1\n    let y\n}"),
            (Code::Expected, 3, 10)
        );
    }

    #[test]
    fn crlf_end_of_line_column_matches_lf() {
        assert_eq!(
            parse_err("fn main() {\r\n    let y\r\n}\r\n"),
            (Code::Expected, 2, 10)
        );
    }

    #[test]
    fn class_with_members() {
        assert_eq!(
            program(
                "class Tree {\n    value: i64\n    mut left: Tree\n\n    fn sum(self) -> i64 { self.value }\n}"
            ),
            "(class Tree (value i64) (mut left Tree) (fn sum (self) i64 (block (. self value))))"
        );
    }

    #[test]
    fn empty_class() {
        assert_eq!(program("class E {}"), "(class E)");
    }

    #[test]
    fn enum_items() {
        let variants = "    A\n    B(i64)\n    C(x: i64, y: [u8])\n";
        for (src, expected) in [
            (
                format!("enum E {{\n{variants}}}"),
                "(enum E A (B i64) (C (x i64) (y [u8])))",
            ),
            (
                format!("enum E {{\n{variants}\n    fn f(self) -> i64 {{ 1 }}\n}}"),
                "(enum E A (B i64) (C (x i64) (y [u8])) (fn f (self) i64 (block 1)))",
            ),
            ("enum E { A }".to_string(), "(enum E A)"),
            ("enum E {}".to_string(), "(enum E)"),
            // A trailing comma ends a payload, as it ends an argument list.
            (
                "enum E {\n    A(i64, E?,)\n}".to_string(),
                "(enum E (A i64 E?))",
            ),
        ] {
            assert_eq!(program(&src), expected, "{src}");
        }
    }

    #[test]
    fn enum_errors() {
        for (src, code, line, col) in [
            ("enum E {\n    A,\n    B\n}", Code::FieldComma, 2, 6),
            (
                "enum E {\n    fn f(self) {}\n    A\n}",
                Code::VariantAfterMethod,
                3,
                5,
            ),
            ("enum E {\n    fn f(mut self) {}\n}", Code::MutSelf, 2, 10),
            ("enum E {\n    None\n}", Code::BuiltInName, 2, 5),
            // Each place a declaration can break.
            ("enum {\n}", Code::Expected, 1, 6),
            ("enum E\n", Code::Expected, 1, 7),
            ("enum E { A B }", Code::Expected, 1, 12),
            ("enum E { 1 }", Code::Expected, 1, 10),
            ("enum E { enum }", Code::Expected, 1, 10),
            ("enum E { A(i64 i64) }", Code::Expected, 1, 16),
            ("enum E {\n    A()\n}", Code::Expected, 2, 7),
            ("class C { enum }", Code::Expected, 1, 11),
        ] {
            assert_eq!(parse_err(src), (code, line, col), "{src}");
        }
    }

    #[test]
    fn enum_broken_method_recovers() {
        let src = "enum E {\n    A\n    fn f(self) {\n        let = 1\n    }\n    fn g(self) -> i64 { 2 }\n}\nfn main() {}";
        let parsed = parsed(src);
        assert_eq!(parsed.diagnostics.len(), 1, "{:?}", parsed.diagnostics);
        assert_eq!(parsed.diagnostics[0].code(), Code::Expected);
        assert_eq!(parsed.program.broken, []);
        assert_eq!(
            parsed.program.to_string(),
            "(enum E A (fn g (self) i64 (block 2)))\n(fn main () unit (block))"
        );
        match parsed.program.items.as_slice() {
            [Item::Enum(e), Item::Function(_)] => {
                assert_eq!(e.broken_methods.len(), 1);
                assert_eq!(e.broken_methods[0].name.as_str(), "f");
            }
            items => panic!("expected an enum and main, found {items:?}"),
        }
    }

    #[test]
    fn broken_enum_keeps_its_name() {
        let parsed = parsed("enum E {\n    None\n}\nfn main() {}");
        assert!(parsed.checkable);
        let [broken] = parsed.program.broken.as_slice() else {
            panic!(
                "expected one broken item, found {:?}",
                parsed.program.broken
            );
        };
        assert_eq!(
            (broken.name.name.as_str(), broken.kind),
            ("E", BrokenKind::Enum)
        );
    }

    #[test]
    fn unnamed_item_ends_at_the_next_enum() {
        // `next_line_item` stops at `enum`, so only the first item is lost.
        let parsed = parsed("class {\n}\nenum E {\n    A\n}");
        assert_eq!(parsed.diagnostics.len(), 1, "{:?}", parsed.diagnostics);
        assert!(!parsed.checkable);
        assert_eq!(parsed.program.to_string(), "(enum E A)");
    }

    #[test]
    fn items_in_any_mix() {
        assert_eq!(
            program("fn f() {}\nclass E {}\nfn g() {}"),
            "(fn f () unit (block))\n(class E)\n(fn g () unit (block))"
        );
    }

    #[test]
    fn method_params_follow_self() {
        assert_eq!(
            program("class A {\n    fn f(self, a: i64, b: bool) {}\n}"),
            "(class A (fn f (self (a i64) (b bool)) unit (block)))"
        );
    }

    #[test]
    fn method_without_self() {
        assert_eq!(
            parse_err("class A {\n    fn f() {}\n}"),
            (Code::MethodNeedsSelf, 2, 8)
        );
    }

    #[test]
    fn self_not_first() {
        assert_eq!(
            parse_err("class A {\n    fn f(x: i64, self) {}\n}"),
            (Code::SelfNotFirst, 2, 18)
        );
    }

    #[test]
    fn self_repeated() {
        assert_eq!(
            parse_err("class A {\n    fn f(self, self) {}\n}"),
            (Code::SelfNotFirst, 2, 16)
        );
    }

    #[test]
    fn self_outside_class() {
        assert_eq!(parse_err("fn f(self) {}"), (Code::SelfNotFirst, 1, 6));
    }

    #[test]
    fn call_of_a_call() {
        assert_eq!(expr_err("f()()"), (Code::Expected, 1, 16));
    }

    #[test]
    fn assign_to_method_call() {
        assert_eq!(expr_err("t.m() = 1"), (Code::AssignToExpression, 1, 13));
    }

    #[test]
    fn bad_class_member() {
        assert_eq!(parse_err("class A { 1 }"), (Code::Expected, 1, 11));
    }

    #[test]
    fn class_members_need_a_line_break() {
        assert_eq!(
            parse_err("class A { x: i64 y: i64 }"),
            (Code::Expected, 1, 18)
        );
    }

    #[test]
    fn optional_types() {
        assert_eq!(
            body("fn main() { let x: Tree? = None }"),
            "(block (let x Tree? None))"
        );
        assert_eq!(
            program("fn f(t: Tree?) -> i64? {}"),
            "(fn f ((t Tree?)) i64? (block))"
        );
        assert_eq!(
            program("class A {\n    mut next: A?\n}"),
            "(class A (mut next A?))"
        );
        // The span runs through the `?`.
        let f = first_fn("fn f() -> Tree? {}");
        assert_eq!(f.ret.expect("has a return type").span, Span::new(10, 15));
    }

    #[test]
    fn if_let_chain() {
        assert_eq!(
            body("fn main() { if let n = node { a } else if let m = other { b } else { c } }"),
            "(block (if-let n node (block a) (block (if-let m other (block b) (block c)))))"
        );
        assert_eq!(
            body("fn main() { if let n = node { a } }"),
            "(block (if-let n node (block a)))"
        );
        assert_eq!(
            body("fn main() { if let n = node { a } else if c { b } }"),
            "(block (if-let n node (block a) (block (if c (block b)))))"
        );
    }

    #[test]
    fn while_let() {
        assert_eq!(
            body("fn main() { while let n = node { f(n) } }"),
            "(block (while-let n node (block (call f n))))"
        );
    }

    #[test]
    fn let_patterns() {
        let rows = [
            (
                "if let Running(c) = s { c } else { 0 }",
                "(if-let (Running c) s (block c) (block 0))",
            ),
            (
                "while let Cons(h, t) = l { f(h) }",
                "(while-let (Cons h t) l (block (call f h)))",
            ),
            ("if let _ = o {}", "(if-let _ o (block))"),
            ("if let x = o {}", "(if-let x o (block))"),
            (
                "if let Expr.Num(n) = e {} else if let Expr.Idle = e {}",
                "(if-let (Expr.Num n) e (block) (block (if-let Expr.Idle e (block))))",
            ),
        ];
        for (src, expected) in rows {
            assert_eq!(expr(src), expected, "{src}");
        }
    }

    #[test]
    fn match_forms() {
        let rows = [
            (
                "match e {\n    Num(n) => n\n    Add(a, b) => a\n    Expr.Num(_) => 0\n    State.Idle => 1\n    _ => 2\n}",
                "(match e (arm (Num n) n) (arm (Add a b) a) (arm (Expr.Num _) 0) \
                 (arm State.Idle 1) (arm _ 2))",
            ),
            (
                "let x = match e {\n    A => 1\n    _ => 2\n}",
                "(let x (match e (arm A 1) (arm _ 2)))",
            ),
            (
                "match s {\n    Running(c) => c.count += 1\n    Idle => x = 2\n    _ => {}\n}",
                "(match s (arm (Running c) (block (+= (. c count) 1))) \
                 (arm Idle (block (= x 2))) (arm _ (block)))",
            ),
            (
                "match o {\n    A => return 0\n    B => return\n    C => break\n    _ => continue\n}",
                "(match o (arm A (block (return 0))) (arm B (block (return))) \
                 (arm C (block (break))) (arm _ (block (continue))))",
            ),
            (
                "match e {\n    A => {\n        f(1)\n        2\n    }\n    _ => 3\n}",
                "(match e (arm A (block (call f 1) 2)) (arm _ 3))",
            ),
            (
                "match e {\n    Add(Num(n), x) => n\n    _ => 0\n}",
                "(match e (arm (Add (Num n) x) n) (arm _ 0))",
            ),
            (
                "match e {\n    Idle() => 1\n    Expr.Idle() => 2\n    Add(a, b,) => 3\n    None => 4\n}",
                "(match e (arm (Idle ()) 1) (arm (Expr.Idle ()) 2) (arm (Add a b) 3) \
                 (arm None 4))",
            ),
            (
                "match a {\n    A => match b {\n        B => 1\n        _ => 2\n    }\n    _ => 3\n}",
                "(match a (arm A (match b (arm B 1) (arm _ 2))) (arm _ 3))",
            ),
            (
                "match f(x).y {\n    _ => 0\n}",
                "(match (. (call f x) y) (arm _ 0))",
            ),
        ];
        for (src, expected) in rows {
            assert_eq!(expr(src), expected, "{src}");
        }
    }

    /// A line that continues with `.` joins the body above it, as everywhere else (ADR 0001).
    #[test]
    fn match_newlines() {
        assert_eq!(
            expr("match e {\n    A => c\n        .count\n    _ => 0\n}"),
            "(match e (arm A (. c count)) (arm _ 0))"
        );
    }

    #[test]
    fn match_syntax() {
        let rows = [
            // A `let` is no arm body.
            ("match e {\n    A => let x = 1\n}", 2, 10),
            // The arrow, the pattern and the variant name are each required.
            ("match e {\n    A 1\n}", 2, 7),
            ("match e {\n    => 1\n}", 2, 5),
            ("match e {\n    E. => 1\n}", 2, 8),
            // One arm per line.
            ("match e {\n    A => 1 B => 2\n}", 2, 12),
            // The braces, the parentheses of a pattern and the value of a `return` close.
            ("match e => 1", 1, 13),
            ("match e {\n    A(a b) => 1\n}", 2, 9),
            ("match e {\n    A => return *\n}", 2, 17),
            ("match e {\n    A { w => 1 }\n}", 2, 11),
            // `::` takes a name, with nothing between its colons.
            ("let c = C::1", 1, 14),
            ("let c = C: :D", 1, 14),
        ];
        for (src, line, col) in rows {
            assert_eq!(
                parse_err(&format!("fn main() {{\n    {src}\n}}")),
                (Code::Expected, line + 1, col),
                "{src}"
            );
        }
    }

    /// Another language's way of writing a `match`, an enum or a pattern, and a comma after an
    /// arm: each at its token, with its fix.
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one row per idiom, so the table reads in one place"
    )]
    fn match_errors() {
        let main = |body: &str| format!("fn main() {{\n{body}\n}}");
        let arm = |arm: &str| main(&format!("    match e {{\n        {arm}\n    }}"));
        // Each row: the source, its code, the position and text of the label, whether the fix
        // is safe, and the source the fix gives.
        let rows = [
            (
                main("    let x = switch e {\n        A => 1\n    }"),
                Code::MatchIdiom,
                (2, 13, "switch"),
                false,
                main("    let x = match e {\n        A => 1\n    }"),
            ),
            (
                main("    switch (e) {\n        A => 1\n    }"),
                Code::MatchIdiom,
                (2, 5, "switch"),
                false,
                main("    match (e) {\n        A => 1\n    }"),
            ),
            (
                main("    return switch e {\n        A => 1\n    }"),
                Code::MatchIdiom,
                (2, 12, "switch"),
                false,
                main("    return match e {\n        A => 1\n    }"),
            ),
            (
                main("    x = switch(e) {\n        A => 1\n    }"),
                Code::MatchIdiom,
                (2, 9, "switch"),
                false,
                main("    x = match(e) {\n        A => 1\n    }"),
            ),
            (
                arm("case A => 1"),
                Code::MatchIdiom,
                (3, 9, "case"),
                false,
                arm("A => 1"),
            ),
            (
                arm("case let .a(x) => 1"),
                Code::MatchIdiom,
                (3, 9, "case"),
                false,
                arm("a(x) => 1"),
            ),
            (
                arm("default: 1"),
                Code::MatchIdiom,
                (3, 9, "default:"),
                true,
                arm("_ => 1"),
            ),
            (
                arm("default:\n            1"),
                Code::MatchIdiom,
                (3, 9, "default:"),
                true,
                arm("_ =>\n            1"),
            ),
            (
                arm("else => 1"),
                Code::MatchIdiom,
                (3, 9, "else"),
                true,
                arm("_ => 1"),
            ),
            (
                arm("A -> 1"),
                Code::MatchIdiom,
                (3, 11, "->"),
                true,
                arm("A => 1"),
            ),
            (
                "enum class Color {\n    Red\n}".to_string(),
                Code::MatchIdiom,
                (1, 6, "class"),
                true,
                "enum Color {\n    Red\n}".to_string(),
            ),
            (
                main("    let x = Color::Red"),
                Code::MatchIdiom,
                (2, 18, "::"),
                true,
                main("    let x = Color.Red"),
            ),
            (
                arm("Color::Red => 1"),
                Code::MatchIdiom,
                (3, 14, "::"),
                true,
                arm("Color.Red => 1"),
            ),
            (
                arm("Rect(w: x, h: y) => 1"),
                Code::PatternForm,
                (3, 14, "w"),
                false,
                arm("Rect(x, y) => 1"),
            ),
            (
                arm("Rect(w:x, h:y) => 1"),
                Code::PatternForm,
                (3, 14, "w"),
                false,
                arm("Rect(x, y) => 1"),
            ),
            (
                arm("Rect(Shape::Wide) => 1"),
                Code::MatchIdiom,
                (3, 19, "::"),
                true,
                arm("Rect(Shape.Wide) => 1"),
            ),
            (
                arm("case .a(x) => 1"),
                Code::MatchIdiom,
                (3, 9, "case"),
                false,
                arm("a(x) => 1"),
            ),
            (
                arm("Rect { w, h } => 1"),
                Code::PatternForm,
                (3, 14, "{ w, h }"),
                false,
                arm("Rect(w, h) => 1"),
            ),
            (
                main("    match e {\n        A => 1,\n        B => 2\n    }"),
                Code::FieldComma,
                (3, 15, ","),
                true,
                main("    match e {\n        A => 1\n        B => 2\n    }"),
            ),
            (
                main("    match e {\n        A => 1, B => 2\n    }"),
                Code::FieldComma,
                (3, 15, ","),
                true,
                main("    match e {\n        A => 1; B => 2\n    }"),
            ),
        ];
        for (src, code, (line, col, text), safe, expected) in rows {
            let d = parse_diagnostic(&src);
            assert_eq!(d.code(), code, "{src}");
            let span = d.span();
            assert_eq!(
                (line_col(&src, span.start()), &src[span.range()]),
                ((line, col), text),
                "{src}"
            );
            let (fix_safe, _) = fix_edits(&d).expect("each row's error has a fix");
            assert_eq!(fix_safe, safe, "{src}");
            assert_eq!(apply_fix(&src, &d), expected, "{src}");
        }
        // A name `switch`, `case` or `default` is still a name where no idiom fits. In a header,
        // whose `{` belongs to the `if`, `switch` is an ordinary name or call.
        for src in [
            "fn main() {\n    let x = switch\n    let y = switch(1) + 1\n}",
            "fn main() {\n    if switch - 1 > 0 {\n    }\n}",
            "fn main() {\n    if switch[0] == 1 {\n    }\n}",
            "fn main() {\n    if switch(1) {\n    }\n}",
            "fn main() {\n    while switch(1) {\n    }\n}",
            "fn main() {\n    match e {\n        case => 1\n        default => 2\n    }\n}",
        ] {
            parse_ok(src);
        }
        // A statement that fails some other way gets that error: another name or call before
        // the `{`, or no `{` after the expression.
        for src in [
            "fn main() {\n    let x = swap e {\n    }\n}",
            "fn main() {\n    swap(e) {\n    }\n}",
            "fn main() {\n    let x = switch e 1\n}",
            "fn main() {\n    switch(e) 1\n}",
        ] {
            assert_eq!(parse_diagnostic(src).code(), Code::Expected, "{src}");
        }
    }

    /// Literals, ranges, alternatives and guards in an arm, and a line that continues.
    #[test]
    fn pattern_forms() {
        let rows = [
            ("0 => 1", "(arm 0 1)"),
            ("-1 => 1", "(arm -1 1)"),
            // The first arm: a later line that starts with `- ` is E0002, as in an expression.
            ("- 1 => 1", "(arm -1 1)"),
            ("-0 => 1", "(arm -0 1)"),
            ("-9223372036854775808 => 1", "(arm -9223372036854775808 1)"),
            ("'a' => 1", "(arm 'a' 1)"),
            ("\"yes\" => 1", "(arm \"yes\" 1)"),
            ("true => 1", "(arm true 1)"),
            ("1..=4 => 1", "(arm (..= 1 4) 1)"),
            ("0..10 => 1", "(arm (.. 0 10) 1)"),
            ("'A'..='Z' => 1", "(arm (..= 'A' 'Z') 1)"),
            ("-9..=-1 => 1", "(arm (..= -9 -1) 1)"),
            ("0..n => 1", "(arm (.. 0 n) 1)"),
            ("LIMIT..=10 => 1", "(arm (..= LIMIT 10) 1)"),
            ("lo..10 => 1", "(arm (.. lo 10) 1)"),
            ("LO..=HI => 1", "(arm (..= LO HI) 1)"),
            ("lo..hi | 0 => 1", "(arm (| (.. lo hi) 0) 1)"),
            ("A | B => 1", "(arm (| A B) 1)"),
            ("0 | 1 | 2 => 1", "(arm (| 0 1 2) 1)"),
            (
                "'a'..='z' | 'A'..='Z' => 1",
                "(arm (| (..= 'a' 'z') (..= 'A' 'Z')) 1)",
            ),
            (
                "Add(Num(0) | Num(1), x) => 1",
                "(arm (Add (| (Num 0) (Num 1)) x) 1)",
            ),
            ("A | B if c => 1", "(arm (| A B) (if c) 1)"),
            ("n if n > 0 => 1", "(arm n (if (> n 0)) 1)"),
            // A line that ends in `|` continues, so these are one arm.
            ("A |\n        B => 1", "(arm (| A B) 1)"),
            (
                "Num(n) =>\n        if c { 1 } else { 2 }",
                "(arm (Num n) (if c (block 1) (block 2)))",
            ),
        ];
        for (arm, want) in rows {
            let src = format!("match e {{\n    {arm}\n}}");
            assert_eq!(expr(&src), format!("(match e {want})"), "{arm}");
        }
    }

    /// The pattern of each arm of `arms`, the arms of a `match` in `main`.
    fn arm_patterns(arms: &str) -> (String, Vec<Pattern>) {
        let src = format!("fn main() {{\n    match e {{\n{arms}\n    }}\n}}");
        let f = first_fn(&src);
        let StmtKind::Expr(Expr {
            kind: ExprKind::Match { arms, .. },
            ..
        }) = &f.body.stmts[0].kind
        else {
            panic!("expected a match");
        };
        let patterns = arms.iter().map(|arm| arm.pattern.clone()).collect();
        (src, patterns)
    }

    /// Each pattern's span covers its own text: `-` and the digits, the whole range, the whole
    /// alternation.
    #[test]
    fn pattern_spans() {
        let (src, patterns) = arm_patterns("        - 1 | 3..=4 | 'a'..n => 1");
        let PatternKind::Or(alternatives) = &patterns[0].kind else {
            panic!("expected alternatives");
        };
        let text = |p: &Pattern| &src[p.span.range()];
        assert_eq!(text(&patterns[0]), "- 1 | 3..=4 | 'a'..n");
        let written: Vec<&str> = alternatives.iter().map(text).collect();
        assert_eq!(written, ["- 1", "3..=4", "'a'..n"]);
    }

    /// A negative literal holds its signed value, the `-`'s span and the digits' span; the
    /// printer shows only the magnitude and the sign, so it cannot tell them apart.
    #[test]
    fn literal_values() {
        let (src, patterns) = arm_patterns(
            "        5 => 1\n        -5 => 1\n        -0 => 1\n        -9223372036854775808 => 1",
        );
        let ints: Vec<_> = patterns
            .iter()
            .map(|p| {
                let PatternKind::Literal(Literal::Int {
                    value,
                    text,
                    negative,
                }) = &p.kind
                else {
                    panic!("expected an integer literal");
                };
                (
                    *value,
                    &src[text.range()],
                    negative.map(|minus| &src[minus.range()]),
                )
            })
            .collect();
        assert_eq!(
            ints,
            [
                (5, "5", None),
                (-5, "5", Some("-")),
                (0, "0", Some("-")),
                (i64::MIN, "9223372036854775808", Some("-")),
            ]
        );
    }

    #[test]
    fn for_inclusive() {
        let rows = [
            ("for i in 1..=3 {}", "(for i (..= 1 3) (block))"),
            ("for i in 0..=n + 1 {}", "(for i (..= 0 (+ n 1)) (block))"),
        ];
        for (src, want) in rows {
            assert_eq!(expr(src), want, "{src}");
        }
    }

    /// The code and position of each error in a pattern, a range or a `|`, and its fix.
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one row per error, so the table reads in one place"
    )]
    fn pattern_parse_errors() {
        let main = |body: &str| format!("fn main() {{\n    {body}\n}}");
        let arm = |arm: &str| main(&format!("match e {{\n        {arm}\n    }}"));
        // Each row: the source, the code and position of the error, and its fix: whether it is
        // safe, and the source it gives.
        let rows = [
            (
                main("for i in 0..<n {}"),
                Code::ForeignRange,
                (2, 15),
                Some((true, main("for i in 0..n {}"))),
            ),
            (
                arm("0..<9 => 1"),
                Code::ForeignRange,
                (3, 10),
                Some((true, arm("0..9 => 1"))),
            ),
            (arm("1...5 => 1"), Code::ForeignRange, (3, 10), None),
            (main("let r = 1...5"), Code::ForeignRange, (2, 14), None),
            // The fix would leave a range outside `for`.
            (main("let r = 0..<3"), Code::ForeignRange, (2, 14), None),
            // A gap after `..` makes the `<` or `.` the end's own first token.
            (main("for i in 0.. <n {}"), Code::Expected, (2, 18), None),
            (main("for i in 0.. .n {}"), Code::Expected, (2, 18), None),
            // A start that is no literal or bare name, and an end that starts a path or a call:
            // at its first token.
            (arm("f(1)..2 => 1"), Code::Expected, (3, 9), None),
            (arm("_..2 => 1"), Code::Expected, (3, 9), None),
            (arm("A.B..2 => 1"), Code::Expected, (3, 9), None),
            (arm("0..f(1) => 1"), Code::Expected, (3, 12), None),
            (arm("0..A.B => 1"), Code::Expected, (3, 12), None),
            // A range of another language is reported whatever its end is.
            (main("let r = 1...*"), Code::ForeignRange, (2, 14), None),
            (main("for i in 0..<* {}"), Code::ForeignRange, (2, 15), None),
            (arm("1...* => 1"), Code::ForeignRange, (3, 10), None),
            (arm("1..<* => 1"), Code::ForeignRange, (3, 10), None),
            // A later arm that starts with `- ` is E0002, as a line of an expression does.
            (
                main("match e {\n        0 => 1\n        - 1 => 2\n    }"),
                Code::LeadingBinaryMinus,
                (4, 9),
                None,
            ),
            (arm("0..* => 1"), Code::Expected, (3, 12), None),
            (arm("-x => 1"), Code::Expected, (3, 10), None),
            // The error of a bound, an alternative, a guard or an end that fails to parse.
            (arm("A | * => 1"), Code::Expected, (3, 13), None),
            (arm("A if * => 1"), Code::Expected, (3, 14), None),
            (main("let r = 1..."), Code::ForeignRange, (2, 14), None),
            (arm("1.5 => 1"), Code::Expected, (3, 9), None),
            (
                arm("9223372036854775808 => 1"),
                Code::IntegerTooLarge,
                (3, 9),
                None,
            ),
            (
                main("let y = a | b"),
                Code::UnexpectedCharacter,
                (2, 15),
                Some((false, main("let y = a || b"))),
            ),
            (
                main("let y = |true"),
                Code::UnexpectedCharacter,
                (2, 13),
                Some((false, main("let y = ||true"))),
            ),
            (main("let r = 0..=3"), Code::RangeOutsideFor, (2, 14), None),
            // A line that starts with `|`, `=>` or a guard's `if` continues the arm above, and
            // the fix joins it, across blank lines too, unless a comment would swallow it.
            (
                arm("A\n        | B => 1"),
                Code::ArmSplit,
                (4, 9),
                Some((true, arm("A | B => 1"))),
            ),
            (
                arm("A\n        => 1"),
                Code::ArmSplit,
                (4, 9),
                Some((true, arm("A => 1"))),
            ),
            (
                arm("A\n        if c => 1"),
                Code::ArmSplit,
                (4, 9),
                Some((true, arm("A if c => 1"))),
            ),
            (
                arm("A if c\n        => 1"),
                Code::ArmSplit,
                (4, 9),
                Some((true, arm("A if c => 1"))),
            ),
            (
                arm("A\n\n        => 1"),
                Code::ArmSplit,
                (5, 9),
                Some((true, arm("A => 1"))),
            ),
            (arm("A // note\n        => 1"), Code::ArmSplit, (4, 9), None),
            // An arm above that is complete has nothing to continue: the line reads as an arm.
            (
                main("match e {\n        A => 1\n        | B => 2\n    }"),
                Code::Expected,
                (4, 9),
                None,
            ),
            (
                main("match e {\n        A => 1\n        => 2\n    }"),
                Code::Expected,
                (4, 9),
                None,
            ),
            (
                main("match e {\n        1 => 1\n        if n > 0 => 1\n    }"),
                Code::Expected,
                (4, 9),
                None,
            ),
            (
                arm("\"{x}\" => 1"),
                Code::InterpolatedPattern,
                (3, 10),
                None,
            ),
            (
                arm("\"a{x:4}b\" => 1"),
                Code::InterpolatedPattern,
                (3, 11),
                None,
            ),
            // An interpolation that does not parse reports that first.
            (arm("\"{*}\" => 1"), Code::Expected, (3, 11), None),
            (arm("\"{x y}\" => 1"), Code::Expected, (3, 13), None),
            // A line that does not continue the arm leaves its `=>` missing.
            (arm("A\n        B => 1"), Code::Expected, (3, 10), None),
        ];
        for (src, code, position, fix) in rows {
            let d = parse_diagnostic(&src);
            assert_eq!(d.code(), code, "{src}");
            assert_eq!(line_col(&src, d.span().start()), position, "{src}");
            let found = fix_edits(&d).map(|(safe, _)| (safe, apply_fix(&src, &d)));
            assert_eq!(found, fix, "{src}");
        }
    }

    /// `_` is no value, and `let _ =` is no arm body, so the error has no fix.
    #[test]
    fn underscore_arm_body_has_no_fix() {
        let src = "fn main() {\n    match e {\n        A => _ = f()\n    }\n}";
        let d = parse_diagnostic(src);
        assert_eq!(d.code(), Code::UnderscoreValue);
        assert_eq!(line_col(src, d.span().start()), (3, 14));
        assert_eq!(fix_edits(&d), None);
    }

    #[test]
    fn let_pattern_needs_a_pattern() {
        let rows = [
            ("if let 1.5 = x {}", 20),
            ("while let 1.5 = x {}", 23),
            ("if let n x {}", 22),
            ("if let = n {}", 20),
            ("if let x = * {}", 24),
        ];
        for (src, col) in rows {
            assert_eq!(expr_err(src), (Code::Expected, 1, col), "{src}");
        }
    }

    #[test]
    fn optionals_do_not_nest() {
        // `Tree??` lexes as one token and `Tree? ?` as two; both name the type `Tree??`.
        let rows = [
            (
                "fn main() { let x: Tree?? = None }",
                Span::new(19, 25),
                (1, 20),
            ),
            ("fn f() -> Tree? ? {}", Span::new(10, 17), (1, 11)),
        ];
        for (src, span, position) in rows {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::OptionalNests, "{src}");
            assert_eq!(d.span(), span, "{src}");
            assert_eq!(line_col(src, d.span().start()), position, "{src}");
        }
    }

    #[test]
    fn array_literals() {
        let rows = [
            ("[]", "(array)"),
            ("[1]", "(array 1)"),
            ("[1, 2,]", "(array 1 2)"),
            ("[\n        1,\n        2,\n    ]", "(array 1 2)"),
            ("[0; n]", "(array-fill 0 n)"),
            ("[[1], []]", "(array (array 1) (array))"),
        ];
        for (src, want) in rows {
            assert_eq!(expr(src), want, "{src}");
        }
    }

    #[test]
    fn indexing_chains_and_binds_like_a_postfix() {
        let rows = [
            ("a[i][j]", "(index (index a i) j)"),
            ("a.b[0].c", "(. (index (. a b) 0) c)"),
            ("[1, 2][0]", "(index (array 1 2) 0)"),
            ("-a[0]", "(- (index a 0))"),
            ("a[i + 1]", "(index a (+ i 1))"),
        ];
        for (src, want) in rows {
            assert_eq!(expr(src), want, "{src}");
        }
    }

    #[test]
    fn for_loops() {
        let rows = [
            ("for x in xs {}", "(for x xs (block))"),
            ("for i in 2..n + 1 {}", "(for i (.. 2 (+ n 1)) (block))"),
            (
                "for i in 0..xs.len() {}",
                "(for i (.. 0 (call (. xs len))) (block))",
            ),
            (
                "for x in [1, 2] { f(x) }",
                "(for x (array 1 2) (block (call f x)))",
            ),
        ];
        for (src, want) in rows {
            assert_eq!(expr(src), want, "{src}");
        }
    }

    #[test]
    fn array_types_round_trip() {
        assert_eq!(
            program("fn f(a: [i64], b: [Node]?, c: [Node?], d: [[bool]]) {}"),
            "(fn f ((a [i64]) (b [Node]?) (c [Node?]) (d [[bool]])) unit (block))"
        );
    }

    #[test]
    fn array_type_spans() {
        let f = first_fn("fn f(a: [[i64]?]?) {}");
        assert_eq!(f.params[0].ty.span, Span::new(8, 17));
    }

    #[test]
    fn syntax_errors_for_arrays_and_for() {
        let rows = [
            ("let r = 0..3", Code::RangeOutsideFor, 22),
            ("f(0..3)", Code::RangeOutsideFor, 16),
            ("for i in 0..1..2 {}", Code::Expected, 26),
            ("[1; 2, 3]", Code::Expected, 18),
            ("(1; 2)", Code::Expected, 15),
            ("a[]", Code::Expected, 15),
        ];
        for (src, code, col) in rows {
            assert_eq!(expr_err(src), (code, 1, col), "{src}");
        }
    }

    #[test]
    fn redundant_semicolon_fix() {
        for (src, position, at, replacement) in [
            ("fn main() {\n    let x = 1;\n}", (2, 14), 25, ""),
            ("fn main() { let x = 1; }", (1, 22), 21, ""),
            (
                "fn main() {\n    print(1); // done\n    print(2)\n}",
                (2, 13),
                24,
                "",
            ),
            ("class A {\n    x: i64;\n}", (2, 11), 20, ""),
            // `return;` has no value.
            ("fn main() { return; }", (1, 19), 18, ""),
            ("fn main() { ; }", (1, 13), 12, ""),
            // Removing the `;` alone would join `}` and `fn`.
            ("fn f() {};fn main() {}", (1, 10), 9, " "),
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::RedundantSemicolon, "{src}");
            assert_eq!(line_col(src, d.span().start()), position, "{src}");
            assert_eq!(
                fix_edits(&d),
                Some((true, vec![(at, at + 1, replacement.to_string())])),
                "{src}"
            );
            assert!(d.help().is_none(), "{src}");
        }
    }

    #[test]
    fn every_redundant_semicolon_fix_at_once() {
        let src = "fn main() {\n    let x = 1;\n    let y = x + 1; // two\n    print(y);\n}\n";
        // A warning breaks no item, so all three `;`s of `main` are reported together.
        let ds = parsed(src).diagnostics;
        let found: Vec<_> = (ds.iter())
            .map(|d| (d.code(), line_col(src, d.span().start())))
            .collect();
        assert_eq!(
            found,
            [(2, 14), (3, 18), (4, 13)].map(|at| (Code::RedundantSemicolon, at))
        );
        let edits: Vec<_> = ds
            .iter()
            .flat_map(|d| d.fixes().iter().flat_map(diagnostic::Fix::edits))
            .collect();
        let fixed = diagnostic::explanations::apply(src, &edits);
        assert_eq!(
            fixed,
            "fn main() {\n    let x = 1\n    let y = x + 1 // two\n    print(y)\n}\n"
        );
        parse_ok(&fixed);
    }

    #[test]
    fn semicolon_mid_line_separates() {
        // A complete statement, then more code on the line.
        for src in [
            "fn main() { let x = 1; let y = 2 }",
            "class A {\n    x: i64; y: i64\n}",
        ] {
            parse_ok(src);
        }
    }

    #[test]
    fn doubled_semicolon_is_redundant_twice() {
        let src = "fn main() { let x = 1;; }";
        let ds = parsed(src).diagnostics;
        let found: Vec<_> = (ds.iter())
            .map(|d| (d.code(), line_col(src, d.span().start())))
            .collect();
        assert_eq!(
            found,
            [(1, 22), (1, 23)].map(|at| (Code::RedundantSemicolon, at))
        );
        for d in &ds {
            assert!(fix_edits(d).is_some_and(|(safe, _)| safe), "{d:?}");
        }
    }

    /// Removing every `;` of a run leaves one space where the run touches code on each side, and
    /// none at the end of a line; removing any one alone also keeps the tokens apart.
    #[test]
    fn a_run_of_redundant_semicolons_leaves_one_space() {
        for (src, alone, all) in [
            (
                "fn main() {\n    let x = 1;;\n}",
                vec!["fn main() {\n    let x = 1;\n}"; 2],
                "fn main() {\n    let x = 1\n}",
            ),
            (
                "fn main() { return;;else { } }",
                vec![
                    "fn main() { return ;else { } }",
                    "fn main() { return;else { } }",
                ],
                "fn main() { return else { } }",
            ),
            // The first `;` separates and stays; the space goes after it.
            (
                "fn main() { a;;b }",
                vec!["fn main() { a; b }"],
                "fn main() { a; b }",
            ),
            (
                "fn f() {};;fn main() {}",
                vec!["fn f() {} ;fn main() {}", "fn f() {};fn main() {}"],
                "fn f() {} fn main() {}",
            ),
        ] {
            let ds: Vec<_> = (parsed(src).diagnostics.into_iter())
                .filter(|d| d.code() == Code::RedundantSemicolon)
                .collect();
            let each: Vec<_> = ds.iter().map(|d| apply_fix(src, d)).collect();
            assert_eq!(each, alone, "{src}");
            assert!(
                (ds.iter()).all(|d| fix_edits(d).is_some_and(|(safe, _)| safe)),
                "{src}"
            );
            let edits: Vec<_> = (ds.iter())
                .flat_map(|d| d.fixes().iter().flat_map(diagnostic::Fix::edits))
                .collect();
            assert_eq!(diagnostic::explanations::apply(src, &edits), all, "{src}");
        }
    }

    #[test]
    fn semicolon_in_an_interpolation_is_an_error() {
        // An interpolation holds one expression: its `;` separates nothing.
        assert_eq!(
            parse_err(r#"fn main() { print("{a; b}") }"#),
            (Code::Expected, 1, 22)
        );
    }

    #[test]
    fn semicolon_after_a_const_separates() {
        // A `const`'s value may go on, so a `;` after it is needed on one line.
        parse_ok("const X = if c { 1 } else { 2 }; fn main() {}");
    }

    #[test]
    fn semicolon_before_a_top_level_brace_is_kept() {
        // One error at the `;`: the `{ }` after it is not taken for a second, broken item.
        assert_eq!(parse_err("fn main(); { }"), (Code::Expected, 1, 10));
        assert_eq!(parse_err("enum E; { A }"), (Code::Expected, 1, 7));
    }

    #[test]
    fn semicolon_after_a_complete_item_separates_from_a_top_level_brace() {
        // After a complete `const` or item the `;` separates, so the error is at the `{`.
        assert_eq!(
            parse_err("const X = 1; { }\nfn main() {}"),
            (Code::Expected, 1, 14)
        );
        assert_eq!(parse_err("fn main() {}; { }"), (Code::Expected, 1, 15));
    }

    #[test]
    fn semicolon_beside_a_continuation() {
        // Removing either `;` would join `x` and `.f()`, so neither is called redundant.
        for (src, at) in [
            ("fn main() {\n    x\n    ; .f()\n}", (3, 5)),
            ("fn main() {\n    x;\n    .f()\n}", (3, 5)),
        ] {
            assert_eq!(parse_err(src), (Code::Expected, at.0, at.1), "{src}");
        }
    }

    #[test]
    fn semicolon_before_else_is_redundant() {
        assert_eq!(
            parse_err("fn main() {\n    if true { print(1) }; else { print(2) }\n}"),
            (Code::RedundantSemicolon, 2, 25)
        );
    }

    /// A join deletes its gap, so over a redundant `;` it would delete that `;`'s W0007 too:
    /// the `else` join is unsafe there, and no arm join is offered.
    #[test]
    fn join_over_a_redundant_semicolon() {
        for (src, semicolon, code, at, safe) in [
            (
                "fn main() {\n    if a {\n    };\n    else {\n    }\n}",
                (3, 6),
                Code::ElseOnNewLine,
                (4, 5),
                Some(false),
            ),
            (
                "fn main() {\n    match e {\n        A;\n        => 1\n    }\n}",
                (3, 10),
                Code::ArmSplit,
                (4, 9),
                None,
            ),
        ] {
            let ds = parsed(src).diagnostics;
            let found: Vec<_> = (ds.iter())
                .map(|d| (d.code(), line_col(src, d.span().start())))
                .collect();
            assert_eq!(
                found,
                [(Code::RedundantSemicolon, semicolon), (code, at)],
                "{src}"
            );
            let safe_join = fix_edits(&ds[1]).map(|(safe, _)| safe);
            assert_eq!(safe_join, safe, "{src}");
        }
    }

    #[test]
    fn semicolon_before_an_arrow_is_not_an_arm_split() {
        assert_eq!(
            parse_err("fn main() { match 1 { 1; => 2 } }"),
            (Code::Expected, 1, 24)
        );
    }

    #[test]
    fn block_value_is_kept_past_a_semicolon() {
        let parsed = parsed("fn main() { let y = { x; } }");
        assert_eq!(
            parsed.program.to_string(),
            "(fn main () unit (block (let y (block x))))"
        );
        let codes: Vec<Code> = parsed.diagnostics.iter().map(Diagnostic::code).collect();
        assert_eq!(codes, [Code::RedundantSemicolon]);
    }

    #[test]
    fn semicolon_elsewhere_is_an_expected_error() {
        // Not after a complete statement: the parser wanted something else there.
        for (src, col) in [
            ("fn main() { let x = ; }", 21),
            ("fn main() { [1; 2; 3] }", 18),
            ("fn main() { f(1; 2) }", 16),
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::Expected, "{src}");
            assert_eq!(line_col(src, d.span().start()), (1, col), "{src}");
            assert!(d.help().is_none(), "{src}");
            assert_eq!(fix_edits(&d), None, "{src}");
        }
    }

    #[test]
    fn optional_array_types_do_not_nest() {
        assert_eq!(
            parse_err("fn f(x: [Node]??) {}"),
            (Code::OptionalNests, 1, 9)
        );
    }

    #[test]
    fn var_is_reserved_everywhere() {
        let var = Code::Var;
        assert_eq!(parse_err("fn main() {\n    var x = 1\n}"), (var, 2, 5));
        assert_eq!(parse_err("fn var() {}"), (var, 1, 4));
        assert_eq!(parse_err("fn main() { let var = 1 }"), (var, 1, 17));
        assert_eq!(parse_err("class P {\n    var hp: i64\n}"), (var, 2, 5));
    }

    #[test]
    fn var_has_a_help_and_no_fix() {
        // The binding and the field each name their own idiom.
        let binding = parse_diagnostic("fn main() {\n    var x = 1\n}");
        let field = parse_diagnostic("class P {\n    var hp: i64\n}");
        for d in [&binding, &field] {
            assert!(d.help().is_some());
            assert_eq!(fix_edits(d), None);
        }
    }

    #[test]
    fn reserved_errors() {
        // One row per place that wants a name or an expression.
        for (src, line, col) in [
            ("fn main() {\n    let type = 1\n}", 2, 9),
            ("class C {\n    record: i64\n}", 2, 5),
            ("fn main() {\n    print(echo)\n}", 2, 11),
            ("fn main() {\n    print(p.type)\n}", 2, 13),
            ("import x", 1, 1),
        ] {
            assert_eq!(parse_err(src), (Code::ReservedWord, line, col), "{src}");
        }
    }

    /// `todo` is a keyword, so where a name goes it is reported as any other keyword is.
    #[test]
    fn todo_is_not_a_name() {
        for (src, line, col) in [
            ("fn main() {\n    let todo = 1\n}", 2, 9),
            ("fn todo() {}", 1, 4),
            ("class todo {}", 1, 7),
        ] {
            assert_eq!(parse_err(src), (Code::Expected, line, col), "{src}");
        }
    }

    #[test]
    fn todo_forms() {
        for (expr, expected) in [
            ("todo", "(todo)"),
            ("todo(\"why\")", "(todo \"why\")"),
            ("todo(\"a\", \"b\")", "(todo \"a\" \"b\")"),
        ] {
            let src = format!("fn main() {{\n    {expr}\n}}");
            assert_eq!(body(&src), format!("(block {expected})"), "{src}");
        }
        // A `todo` that ends the file has fewer than three tokens after it.
        parse_ok("fn main() {}\nconst X = todo");
        // The arguments are a call's, with its errors.
        assert_eq!(
            parse_err("fn main() {\n    todo(1 2)\n}"),
            (Code::Expected, 2, 12)
        );
    }

    /// Each other way of writing `todo`: the error's position, and the line its fix makes.
    #[test]
    fn todo_misspelled() {
        for (line, col, after) in [
            ("todo()", 9, "todo"),
            ("todo!", 9, "todo"),
            ("todo!(\"why\")", 9, "todo(\"why\")"),
            ("todo!()", 9, "todo"),
            ("(todo!)", 10, "(todo)"),
            ("(todo!(\"why\"))", 10, "(todo(\"why\"))"),
        ] {
            let src = format!("fn main() {{\n    {line}\n}}");
            assert_eq!(parse_err(&src), (Code::TodoForm, 2, col), "{src}");
            assert_eq!(fixed(&src), format!("fn main() {{\n    {after}\n}}"));
        }
    }

    /// A removal that would join `todo` to the word after it, as `todo!x` to `todox`, is an
    /// unsafe fix; a symbol after it keeps the fix safe.
    #[test]
    fn todo_fix_before_a_word_is_unsafe() {
        for (line, safe) in [
            ("todo!x", false),
            ("todo!0e", false),
            ("todo()_", false),
            ("todo!()y", false),
            ("todo!+1", true),
        ] {
            let src = format!("fn main() {{\n    {line}\n}}");
            let d = parse_diagnostic(&src);
            assert_eq!(d.code(), Code::TodoForm, "{src}");
            assert_eq!(fix_edits(&d).map(|(s, _)| s), Some(safe), "{src}");
        }
    }

    /// The line check looks only at the breaks between the keyword and the `!`, however many
    /// breaks come before the keyword.
    #[test]
    fn todo_bang_after_many_lines() {
        let src = "fn main() {\n\n\n\n    todo!\n}";
        assert_eq!(parse_err(src), (Code::TodoForm, 5, 9), "{src}");
    }

    /// `.config/nextest.toml` kills this test after 30 s, matching it by name: a rename must
    /// update the override there. Each `todo!` once scanned the file's line breaks, so many of
    /// them were quadratic.
    #[test]
    fn many_todo_bangs_parse_in_linear_time() {
        const ITEMS: usize = 100_000;
        let parsed = parsed(&"fn f() {\n    todo!\n}\n".repeat(ITEMS));
        assert_eq!(parsed.diagnostics.len(), ITEMS);
        assert!(
            parsed
                .diagnostics
                .iter()
                .all(|d| d.code() == Code::TodoForm)
        );
    }

    /// A call `switch(...)` is parsed once: a parser that tried C's `switch e {` first and
    /// then parsed the call again would take time exponential in the nesting, and this test
    /// would hang instead of failing.
    #[test]
    fn nested_switch_calls_parse_in_linear_time() {
        const DEPTH: usize = 32;
        parse_ok(&format!(
            "fn main() {{\n    let x = {}1{}\n}}\n",
            "switch(".repeat(DEPTH),
            ")".repeat(DEPTH)
        ));
    }

    /// Inside parentheses no line break ends `todo`, so it takes the message on the next line.
    /// Outside them the break ends it (`todo_ends_at_the_line` in the checker's tests).
    #[test]
    fn todo_takes_a_message_on_the_next_line_in_parentheses() {
        assert_eq!(
            body("fn main() {\n    (todo\n     (\"why\"))\n}"),
            "(block (todo \"why\"))"
        );
    }

    /// A `!` on a later line is not part of `todo`, even inside parentheses, where no line break
    /// ends it: the `todo` is bare, and the `!` is where its `)` should be.
    #[test]
    fn todo_bang_on_the_next_line() {
        for rest in ["!", "!()", "!(\"why\")"] {
            let src = format!("fn main() {{\n    (todo\n     {rest})\n}}");
            assert_eq!(parse_err(&src), (Code::Expected, 3, 6), "{src}");
        }
    }

    #[test]
    fn mut_before_a_reserved_word_is_still_reported() {
        assert_eq!(parse_err("fn f(mut type: i64) {}"), (Code::Expected, 1, 6));
    }

    #[test]
    fn mut_is_a_keyword() {
        assert_eq!(
            parse_err("fn main() { let mut = 1 }"),
            (Code::Expected, 1, 21)
        );
    }

    #[test]
    fn one_line_class() {
        assert_eq!(
            program("class UserId { value: i64 }"),
            "(class UserId (value i64))"
        );
    }

    #[test]
    fn field_let_fix() {
        // The rest of the line is a field: `let ` goes, up to the next token.
        for (src, position, edit) in [
            ("class P {\n    let hp: [Tree?]\n}", (2, 5), (14, 18)),
            ("class P {\n    let mut hp: i64\n}", (2, 5), (14, 18)),
            ("class P { let hp: i64 }", (1, 11), (10, 14)),
            ("class P {\n    let   hp: i64 // note\n}", (2, 5), (14, 20)),
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::FieldLet, "{src}");
            assert_eq!(line_col(src, d.span().start()), position, "{src}");
            assert_eq!(
                fix_edits(&d),
                Some((true, vec![(edit.0, edit.1, String::new())])),
                "{src}"
            );
            assert!(d.help().is_none(), "{src}");
        }
    }

    #[test]
    fn safe_field_let_fix_removes_the_error() {
        let src = "class P {\n    let mut hp: i64\n    name: [i64]\n}\n";
        let result = fixed(src);
        assert_eq!(result, "class P {\n    mut hp: i64\n    name: [i64]\n}\n");
        parse_ok(&result);
    }

    #[test]
    fn let_field_with_anything_else_is_a_help() {
        for (src, position) in [
            // Not a field, with and without `mut`.
            ("class P {\n    let hp = 1\n}", (2, 5)),
            ("class C { let mut n = 0 }", (1, 11)),
            ("class P {\n    let\n}", (2, 5)),
            // A field, but the line goes on.
            ("class P {\n    let hp: i64, y: i64\n}", (2, 5)),
            ("class P {\n    let hp: i64 y\n}", (2, 5)),
            ("class P {\n    let mut hp: i64 y\n}", (2, 5)),
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::FieldLet, "{src}");
            assert_eq!(line_col(src, d.span().start()), position, "{src}");
            assert!(d.help().is_some(), "{src}");
            assert_eq!(fix_edits(&d), None, "{src}");
        }
    }

    #[test]
    fn comma_between_members() {
        // The fix is safe: it deletes a comma that ends its line, and replaces any other with `;`.
        for (src, at, expected) in [
            (
                "class P {\n    x: i64, y: i64\n}",
                (2, 11),
                "class P {\n    x: i64; y: i64\n}",
            ),
            (
                "enum E {\n    A,\n    B\n}",
                (2, 6),
                "enum E {\n    A\n    B\n}",
            ),
            ("enum E { A, B }", (1, 11), "enum E { A; B }"),
            ("enum E { A,   B(i64) }", (1, 11), "enum E { A;   B(i64) }"),
            ("enum E { A, }", (1, 11), "enum E { A }"),
            // A `;` after the comma already separates.
            (
                "class C { x: i64,; y: i64 }",
                (1, 17),
                "class C { x: i64; y: i64 }",
            ),
            (
                "enum E {\n    A, // c\n    B\n}",
                (2, 6),
                "enum E {\n    A // c\n    B\n}",
            ),
            (
                "enum E {\n    A,  \n    B\n}",
                (2, 6),
                "enum E {\n    A  \n    B\n}",
            ),
            (
                "enum E {\n    A(i64),\n    B\n}",
                (2, 11),
                "enum E {\n    A(i64)\n    B\n}",
            ),
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::FieldComma, "{src}");
            assert_eq!(line_col(src, d.span().start()), at, "{src}");
            assert!(d.help().is_some(), "{src}");
            assert_eq!(fixed(src), expected, "{src}");
            parse_ok(expected);
        }
    }

    #[test]
    fn comma_fix_parses_clean() {
        // Each fix, applied to the one comma error, leaves a program that parses clean.
        for src in [
            "class P { x: i64, y: i64 }",
            "fn main() { match 1 { 0 => 1, _ => 2 } }",
            "class P {\n    x: i64,\n    y: i64\n}",
        ] {
            parse_ok(&fixed(src));
        }
        // A comma before `}` goes, because a `;` there would be a redundant one.
        let src = "enum E { A, B, }";
        assert_eq!(parse_err(src), (Code::FieldComma, 1, 11));
        let src = fixed(src);
        assert_eq!(parse_err(&src), (Code::FieldComma, 1, 14));
        parse_ok(&fixed(&src));
    }

    #[test]
    fn comma_fix_keeps_the_text_the_lexer_skipped() {
        // The `$` is a lexer error; the fix must not delete it.
        let src = "enum E { A, $B }";
        let parsed = parsed(src);
        let comma = parsed
            .diagnostics
            .iter()
            .find(|d| d.code() == Code::FieldComma)
            .expect("a comma error");
        assert_eq!(apply_fix(src, comma), "enum E { A; $B }");
        let lexed = lex(src).expect("source lexes");
        assert!(
            lexed
                .diagnostics
                .iter()
                .any(|d| d.code() == Code::UnexpectedCharacter)
        );
    }

    #[test]
    fn comma_after_a_method_is_not_a_field_comma() {
        assert_eq!(
            parse_err("class C {\n fn f(self) {},\n}"),
            (Code::Expected, 2, 15)
        );
    }

    #[test]
    fn mut_self_fix() {
        // Safe only when `self` ends the parameter: `mut self: i64` stays broken without `mut`.
        for (src, edit, safe) in [
            ("class C {\n    fn f(mut self) {}\n}", (19, 23), true),
            (
                "class C {\n    fn f(mut   self, n: i64) {}\n}",
                (19, 25),
                true,
            ),
            ("class C {\n    fn f(mut self: i64) {}\n}", (19, 23), false),
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::MutSelf, "{src}");
            assert_eq!(line_col(src, d.span().start()), (2, 10), "{src}");
            assert_eq!(
                fix_edits(&d),
                Some((safe, vec![(edit.0, edit.1, String::new())])),
                "{src}"
            );
            assert!(d.help().is_some(), "{src}");
        }
    }

    #[test]
    fn safe_mut_self_fix_removes_the_error() {
        let src = "class C {\n    mut n: i64\n    fn add(mut self, by: i64) { self.n += by }\n}\n";
        let result = fixed(src);
        assert_eq!(
            result,
            "class C {\n    mut n: i64\n    fn add(self, by: i64) { self.n += by }\n}\n"
        );
        parse_ok(&result);
    }

    #[test]
    fn mut_self_in_an_enum_is_an_unsafe_fix() {
        // Whatever follows `self`, the body may still assign it, so the fix is never safe.
        for src in [
            "enum E {\n    fn f(mut self) {}\n}",
            "enum E {\n    fn f(mut self, n: i64) {}\n}",
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::MutSelf, "{src}");
            assert_eq!(line_col(src, d.span().start()), (2, 10), "{src}");
            assert!(d.help().is_some(), "{src}");
            assert_eq!(apply_fix(src, &d), src.replace("mut self", "self"), "{src}");
            assert!(fix_edits(&d).is_some_and(|(safe, _)| !safe), "{src}");
        }
    }

    #[test]
    fn mut_self_outside_the_receiver_position() {
        assert_eq!(parse_err("fn f(mut self) {}"), (Code::SelfNotFirst, 1, 10));
        assert_eq!(
            parse_err("class C {\n    fn f(self, n: i64, mut self) {}\n}"),
            (Code::SelfNotFirst, 2, 28)
        );
    }

    #[test]
    fn mut_parameter() {
        let src = "fn f(a: i64, mut n: i64) {}";
        let d = parse_diagnostic(src);
        assert_eq!(d.code(), Code::MutParameter);
        assert_eq!(line_col(src, d.span().start()), (1, 14));
        // Removing the `mut` would make the body assign to a parameter.
        assert!(d.help().is_some());
        assert_eq!(fix_edits(&d), None);
    }

    #[test]
    fn mut_before_neither_self_nor_a_name() {
        assert_eq!(parse_err("fn f(mut 1) {}"), (Code::Expected, 1, 6));
    }

    #[test]
    fn broken_items_keep_their_names() {
        let src = "fn f() {\n    let = 1\n}\nclass C {\n    x i64\n}\nclass D {\n    fn m(self) {\n        let = 1\n    }\n}\n";
        let parsed = parsed(src);
        assert!(parsed.checkable);
        assert_eq!(parsed.diagnostics.len(), 3, "{:?}", parsed.diagnostics);
        let ident = |name: &str, start| Ident {
            name: Symbol::intern(name),
            span: Span::new(start, start + 1),
        };
        assert_eq!(
            parsed.program.broken,
            [
                Broken {
                    name: ident("f", 3),
                    kind: BrokenKind::Function
                },
                Broken {
                    name: ident("C", 29),
                    kind: BrokenKind::Class
                }
            ]
        );
        match parsed.program.items.as_slice() {
            [Item::Class(d)] => {
                assert_eq!(d.members, []);
                assert_eq!(d.broken_methods, [ident("m", 62)]);
            }
            items => panic!("expected class D alone, found {items:?}"),
        }
    }

    #[test]
    fn earliest_error_in_an_item_wins() {
        // The parse error comes first here.
        assert_eq!(
            parse_err("fn main() {\n    let = 1 $\n}\n"),
            (Code::Expected, 2, 9)
        );
        // On a tie, the lexer's error wins: it may have caused the parse error.
        assert_eq!(
            parse_err("fn main() {\n    let 99999999999999999999 = 1\n}\n"),
            (Code::IntegerTooLarge, 2, 9)
        );
    }

    #[test]
    fn lexer_error_outside_every_item_stays() {
        let parsed = parsed("fn f() {} $\nfn main() {}\n");
        let codes: Vec<_> = parsed.diagnostics.iter().map(Diagnostic::code).collect();
        assert_eq!(codes, [Code::UnexpectedCharacter]);
        assert!(parsed.checkable);
        assert_eq!(parsed.program.broken, []);
        assert_eq!(parsed.program.items.len(), 2);
    }

    #[test]
    fn item_cut_off_by_end_of_file() {
        assert_eq!(
            parse_err("fn main() {}\nfn f() ->"),
            (Code::Expected, 2, 10)
        );
    }

    /// `**` is Python's, and an error here, at the `**`. The unsafe fix is `l.pow(r)`, offered only
    /// when both sides are plain operands, and never for an assignment or a second `**`.
    #[test]
    fn power_operator() {
        let wrap = |expr: &str| format!("fn main() {{\n    let _ = {expr}\n}}");
        let rows = [
            ("x ** 2", Some("x.pow(2)")),
            ("x ** y", Some("x.pow(y)")),
            ("2.0 ** x", Some("2.0.pow(x)")),
            ("a.b ** f(c)", Some("a.b.pow(f(c))")),
            ("a.m() ** xs[0]", Some("a.m().pow(xs[0])")),
            ("(a + b) ** 2", Some("(a + b).pow(2)")),
            ("((a) + b) ** 2", Some("((a) + b).pow(2)")),
            ("x ** (a + b)", Some("x.pow((a + b))")),
            ("2 ** 10", None),
            ("None ** 2", None),
            ("x ** 2 ** 3", None),
            ("x ** -1", None),
            ("-x ** 2", None),
            // `**` binds as `*` does, so only its own operands matter.
            ("a + b ** 2", Some("a + b.pow(2)")),
            ("x ** a + b", Some("x.pow(a) + b")),
            // Parentheses around a whole product are not around its last factor.
            ("(a) * (b) ** 2", None),
            ("a * (b) ** 2", None),
        ];
        for (expr, fixed) in rows {
            let src = wrap(expr);
            let d = parse_diagnostic(&src);
            assert_eq!(d.code(), Code::PowerOperator, "{src}");
            let (line, col) = line_col(&src, d.span().start());
            let found = &src[d.span().range()];
            assert_eq!(
                (line, col, found),
                (2, 13 + expr.find("**").expect("has `**`"), "**"),
                "{src}"
            );
            match fixed {
                Some(fixed) => {
                    let (safe, _) = fix_edits(&d).expect("the error has a fix");
                    assert!(!safe, "{src}");
                    assert_eq!(apply_fix(&src, &d), wrap(fixed));
                    parse_ok(&wrap(fixed));
                }
                None => assert!(fix_edits(&d).is_none(), "{src}"),
            }
        }
        // `**` where no operand starts, and no right side, are the parser's own errors.
        assert_eq!(
            parse_err("fn main() {\n    let ** = 1\n}"),
            (Code::Expected, 2, 9)
        );
        assert_eq!(
            parse_err("fn main() {\n    let _ = x **\n}"),
            (Code::Expected, 3, 1)
        );
        // An assignment is no `pow` call, and `=` after `**` is not reported again.
        let src = "fn main() {\n    let mut x = 1.0\n    x **= 2\n}";
        let d = parse_diagnostic(src);
        assert_eq!(d.code(), Code::PowerOperator);
        assert_eq!(line_col(src, d.span().start()), (3, 7));
        assert!(fix_edits(&d).is_none());
    }
}
