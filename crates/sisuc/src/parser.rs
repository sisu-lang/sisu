//! The parser: tokens to the syntax tree.

use std::borrow::Cow;
use std::cell::OnceCell;
use std::cmp::Ordering;
use std::iter;
use std::mem;
use std::ops::Range;

use crate::ast::{
    AliasDecl, Align, Applied, Arg, Arm, AssocChoice, AssocType, BinaryOp, Block, Bound, Broken,
    BrokenKind, Class, CompareOp, Const, Enum, Expr, ExprKind, Extend, FieldDecl, ForSource,
    FormatSpec, Function, Ident, Interface, Item, Literal, Member, MutSelf, Param, Part, Pattern,
    PatternKind, PayloadField, Program, Range as RangeExpr, Requirement, Stmt, StmtKind, Struct,
    TypeExpr, TypeKind, TypeParam, UnaryOp, Variant,
};
use crate::diagnostic::{
    self, AnyFormKind, BuiltIn, ClashForm, ColonBound, Diagnostic, DiagnosticKind, ElseOf,
    ForeignPattern, ForeignRange, InterfaceBodyForm, Join, LetField, MatchIdiomForm, NestingForm,
    Owner, PowerFix, SpecForm, SpecLimit,
};
use crate::lexer::{Lexed, MIN_MAGNITUDE, Token, TokenKind, literal_too_large};
use crate::source::{Source, Span, removal_end};
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
/// it and reads its part without a spec. A lexer warning breaks nothing. The prelude (`prelude`)
/// may declare a variant `None`, which no other source may.
// `scripts/scaling.py` reads this function's instruction count.
#[inline(never)]
pub(crate) fn parse(lexed: Lexed<'_>, prelude: bool) -> Parsed {
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
    let halves = OnceCell::new();
    Parser {
        source,
        tokens: &tokens,
        window: &tokens,
        halves: &halves,
        halved: None,
        scrutinee: None,
        pos: 0,
        depth: Height::default(),
        lexer: &diagnostics,
        comments: &comments,
        breaks: &breaks,
        recovered: Vec::new(),
        header: false,
        pattern_place: PatternPlace::Arm,
        warnings,
        labels: Vec::new(),
        loops: Vec::new(),
        prelude,
        struct_self: false,
    }
    .program()
}

/// The program of `source`, which must lex and parse without a diagnostic.
#[cfg(test)]
pub(crate) fn parse_ok(source: &str) -> Program {
    let parsed = parse(
        crate::lexer::lex(source).expect("the source should lex"),
        false,
    );
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

/// The format spec `text`, written at `at` from its `:`: `#`, or `[<^>][width][.precision]`.
/// A width is from 1 to 1000. A precision is from 0 to 30.
/// A single `#` beside those parts, in any place, is an error of its own form.
/// So is a `?` after a spec that is right without it: `?` alone, `#?` alone, and `>5?`.
/// The spec is read without the `#` first, so a spec that is wrong without it gets that error.
/// A limit error is the exception: the `#` is the first thing to remove then.
fn format_spec(text: &str, at: Span) -> Result<FormatSpec, Diagnostic> {
    if let Some(without) = text.strip_suffix('?') {
        let form = match without {
            "" => Some(SpecForm::Debug),
            "#" => Some(SpecForm::PrettyDebug),
            _ if plain_format_spec(without, at).is_ok() => Some(SpecForm::DebugSpec),
            _ => None,
        };
        if let Some(form) = form {
            return Err(DiagnosticKind::UnsupportedFormatSpec { at, form }.into());
        }
    }
    if text.matches('#').count() == 1 {
        let without = text.replacen('#', "", 1);
        if without.is_empty() {
            return Ok(FormatSpec {
                align: None,
                width: None,
                precision: None,
                pretty: true,
                span: at,
            });
        }
        return Err(match plain_format_spec(&without, at) {
            Err(error) if error.code() != diagnostic::Code::FormatSpecLimit => error,
            result => DiagnosticKind::UnsupportedFormatSpec {
                at,
                form: SpecForm::Pretty(result.err().map(|_| spec_limit(&without))),
            }
            .into(),
        });
    }
    plain_format_spec(text, at)
}

/// The parts of the plain spec `text`, which has the right shape: the alignment, the digits of
/// the width, the digits of the precision if a `.` is there, and what is left over.
fn split_spec(text: &str) -> (Option<Align>, &str, Option<&str>, &str) {
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
    (align, width, precision, rest)
}

/// A width from 1 to 1000, or `None` when `digits` are empty, too many, or too large.
fn width_in_limit(digits: &str) -> Option<u16> {
    digits.parse::<u16>().ok().filter(|&w| w <= 1000)
}

/// A precision from 0 to 30, or `None` when `digits` are empty, too many, or too large.
fn precision_in_limit(digits: &str) -> Option<u8> {
    digits.parse::<u8>().ok().filter(|&p| p <= 30)
}

/// Which limits the plain spec `text` passes. `text` is a spec that `plain_format_spec` rejects
/// with a limit error, so at least one part passes.
fn spec_limit(text: &str) -> SpecLimit {
    let (_, width, precision, _) = split_spec(text);
    let wide = !width.is_empty() && width_in_limit(width).is_none();
    let fine = precision.is_some_and(|p| precision_in_limit(p).is_none());
    match (wide, fine) {
        (true, true) => SpecLimit::Both,
        (true, false) => SpecLimit::Width,
        _ => SpecLimit::Precision,
    }
}

/// The format spec `text` without a `#`: `[<^>][width][.precision]`.
fn plain_format_spec(text: &str, at: Span) -> Result<FormatSpec, Diagnostic> {
    let (align, width, precision, rest) = split_spec(text);
    if !rest.is_empty() || width.starts_with('0') {
        return Err(DiagnosticKind::UnsupportedFormatSpec {
            at,
            form: SpecForm::Plain,
        }
        .into());
    }
    if precision.is_some_and(str::is_empty) || (width.is_empty() && precision.is_none()) {
        return Err(DiagnosticKind::MalformedFormatSpec { at }.into());
    }
    let limit = |width| Diagnostic::from(DiagnosticKind::FormatSpecLimit { at, width });
    let width = match width {
        "" => None,
        w => Some(width_in_limit(w).ok_or_else(|| limit(true))?),
    };
    let precision = match precision {
        None => None,
        Some(p) => Some(precision_in_limit(p).ok_or_else(|| limit(false))?),
    };
    Ok(FormatSpec {
        align,
        width,
        precision,
        pretty: false,
        span: at,
    })
}

/// How deep the syntax tree may nest. Every later stage walks the tree recursively, and so does
/// dropping it, so the parser is the one place that bounds the stack they need.
const MAX_DEPTH: usize = 256;

/// The height of a path through the tree: its levels, and how many of them are links of a chain
/// or `else if`s. `Ord` puts the deeper path first, so the maximum of two heights is the deeper
/// path with its own counts, and the report can name what made that path deep. Two heights with the
/// same counts are equally deep, so the order and the equality ignore `run` and `longest`.
#[derive(Clone, Copy, Default)]
struct Height {
    /// The levels on the path.
    depth: usize,
    /// The levels that an infix operator, a postfix step or a prefix operator added.
    links: usize,
    /// The levels that an `else if` added.
    else_ifs: usize,
    /// The `else if`s in a row that end the path. A level of another kind ends the run.
    run: usize,
    /// The longest run of `else if`s on the path, with its length and its last `else if`, from
    /// `else` to `if`. Of two runs of the same length, the later one.
    longest: Option<(usize, Span)>,
}

impl Height {
    /// The counts, which are all that the order and the equality of heights compare.
    fn counts(self) -> (usize, usize, usize) {
        (self.depth, self.links, self.else_ifs)
    }

    /// One level more.
    fn deeper(self) -> Self {
        Self {
            depth: self.depth + 1,
            run: 0,
            ..self
        }
    }

    /// One level more, which a link of a chain adds.
    fn link(self) -> Self {
        Self {
            links: self.links + 1,
            ..self.deeper()
        }
    }

    /// One level more, which the `else if` at `at` adds.
    fn else_if(self, at: Span) -> Self {
        let run = self.run + 1;
        let longest = match self.longest {
            Some((len, _)) if len > run => self.longest,
            _ => Some((run, at)),
        };
        Self {
            else_ifs: self.else_ifs + 1,
            run,
            longest,
            ..self.deeper()
        }
    }

    /// What made the path deep: links when they are more than half its levels, else `else if`s
    /// when they are, else nesting.
    fn form(self) -> NestingForm {
        if 2 * self.links > self.depth {
            NestingForm::Chain
        } else if 2 * self.else_ifs > self.depth {
            NestingForm::ElseIf
        } else {
            NestingForm::Tree
        }
    }
}

impl PartialEq for Height {
    fn eq(&self, other: &Self) -> bool {
        self.counts() == other.counts()
    }
}

impl Eq for Height {}

impl PartialOrd for Height {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Height {
    fn cmp(&self, other: &Self) -> Ordering {
        self.counts().cmp(&other.counts())
    }
}

#[cfg(test)]
thread_local! {
    /// How many tokens `type_args_follow` has visited on this thread.
    static SCANNED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

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

/// The bracket depth after a token of `kind` at bracket depth `depth`: `(`, `[`, `{` and the head
/// of an interpolation open one level, and `)`, `]`, `}` and the tail of an interpolation close
/// one. The middle of an interpolation closes one and opens the next, so it keeps the depth.
/// `None` when the token closes a bracket that the scan did not open. The error-path scans of
/// the parser share this rule.
fn nested(kind: &TokenKind, depth: usize) -> Option<usize> {
    match kind {
        TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace | TokenKind::StrHead(_) => {
            Some(depth + 1)
        }
        TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace | TokenKind::StrTail(_) => {
            depth.checked_sub(1)
        }
        _ => Some(depth),
    }
}

/// Whether `kind` begins a postfix link (`.`, `?.` or `[`); `postfix` and `at_min_literal`
/// share it.
fn starts_postfix(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Dot | TokenKind::QuestionDot | TokenKind::LBracket
    )
}

/// Whether an end follows the `..` or `..=` that was just read, when the next token is `kind`:
/// an operand does. So does a `{`, a block expression, except in the head of a `for`, `if`,
/// `while` or `match`, where `header` holds and the `{` opens the body. An integer literal does
/// too, so that one too large for an `i64` gets its own error, not a range with no end.
fn range_end_starts(kind: &TokenKind, header: bool) -> bool {
    starts_operand(kind)
        || (*kind == TokenKind::LBrace && !header)
        || matches!(kind, TokenKind::Int(_))
}

/// The `return` or `break` at the right end of `e` whose value is a block `{ ... }` that ends at
/// byte `end`, the end of the header: the rightmost operand, through binary operators such as
/// `??`. A jump takes everything to its right, so in a header it takes the body block when it has
/// no value of its own. A `)` after the block ends the jump, so the block is then no body.
fn block_taking_jump(e: &Expr, end: usize) -> Option<&Expr> {
    if let ExprKind::Return {
        value: Some(value), ..
    }
    | ExprKind::Break {
        value: Some(value), ..
    } = &e.kind
        && matches!(&value.kind, ExprKind::Block(block) if block.span.end() == end)
    {
        return Some(e);
    }
    right_operand(e).and_then(|last| block_taking_jump(last, end))
}

/// The operand at the right end of `e`: what a binary or unary operator, or a jump, takes last.
fn right_operand(e: &Expr) -> Option<&Expr> {
    match &e.kind {
        ExprKind::Return {
            value: Some(last), ..
        }
        | ExprKind::Break {
            value: Some(last), ..
        }
        | ExprKind::Binary { rhs: last, .. }
        | ExprKind::Coalesce { rhs: last, .. }
        | ExprKind::Is { rhs: last, .. }
        | ExprKind::Unary { operand: last, .. } => Some(last),
        ExprKind::Range(range) => range.end.as_deref(),
        ExprKind::Compare { operands, .. } => operands.last(),
        _ => None,
    }
}

/// Whether `e` ends in an `if` that has no `else` yet, so that an `else` after `e` is the `if`'s.
/// An `else if` chain is as open as its last `if`.
fn ends_in_open_if(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::If { else_block, .. } | ExprKind::IfLet { else_block, .. } => {
            match else_block {
                None => true,
                Some(block) => match &block.stmts[..] {
                    // The parser gives an `else if` the span of its inner `if`, and an
                    // `else { if ... }` the span of its braces.
                    [stmt] if stmt.span == block.span => {
                        matches!(&stmt.kind, StmtKind::Expr(inner) if ends_in_open_if(inner))
                    }
                    _ => false,
                },
            }
        }
        _ => right_operand(e).is_some_and(ends_in_open_if),
    }
}

/// Whether `kind` begins an operand that `unary` and `primary` take. A `_`, a leading `.` and
/// the magnitude of `i64::MIN` start no operand: they are errors there. A `{` does not count:
/// after an operator it opens the body of an `if`, `while`, `for` or `match` more often than a
/// block operand. A fix that writes an operator before such a token would swap one error for
/// another.
fn starts_operand(kind: &TokenKind) -> bool {
    match kind {
        TokenKind::Int(magnitude) => i64::try_from(*magnitude).is_ok(),
        _ => matches!(
            kind,
            TokenKind::Float(_)
                | TokenKind::Char(_)
                | TokenKind::Str(_)
                | TokenKind::StrHead(_)
                | TokenKind::Ident(_)
                | TokenKind::True
                | TokenKind::False
                | TokenKind::SelfKw
                | TokenKind::NoneKw
                | TokenKind::Todo
                | TokenKind::LParen
                | TokenKind::LBracket
                | TokenKind::If
                | TokenKind::Loop
                | TokenKind::Match
                | TokenKind::Return
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Minus
                | TokenKind::Bang
        ),
    }
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
    Assoc(AssocChoice),
}

/// One line of a class body.
enum ClassLine {
    Member(Member),
    Assoc(AssocChoice),
}

/// One line of an interface body.
enum InterfaceLine {
    Requirement(Requirement),
    Assoc(AssocType),
}

/// One line of an `extend` block.
enum ExtendLine {
    Method(Function),
    Assoc(AssocChoice),
}

/// A function's signature as written: everything before its body.
struct Signature {
    /// The `fn` token.
    fn_span: Span,
    name: Ident,
    type_params: Vec<TypeParam>,
    angles: Span,
    self_param: Option<Span>,
    mut_self: Option<MutSelf>,
    params: Vec<Param>,
    ret: Option<TypeExpr>,
}

/// What a function's first parameter may be: where the function stands.
#[derive(Clone, Copy)]
enum Receiver<'o> {
    /// A function outside a class, an enum, an interface and an `extend` block: no `self`.
    Free,
    /// A method of a class, an enum or an interface. `mut self` is an error at once.
    Of(&'o Owner),
    /// A method of an `extend` block. The target may be a class or an enum, which the parser
    /// cannot tell, so it records `mut self` for the checker to report.
    Extension,
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

/// Where a pattern stands, which says what can end it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PatternPlace {
    /// The pattern of a match arm.
    Arm,
    /// The pattern of a `let`, an `if let` or a `while let`.
    Let,
    /// A pattern inside `( )` or `{ }` of another pattern.
    Sub,
}

/// What the tokens after `let` and `mut` read as (`Parser::let_head`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum LetHead {
    /// A name, which `binding` reads, or whatever is not a pattern.
    Name,
    /// A pattern that is not a plain name.
    Pattern,
    /// `Enum.Variant` and then `=`: a pattern when an `else` follows the value, else a name
    /// that `.` ends.
    Qualified,
}

struct Parser<'a> {
    /// The text the token spans index.
    source: Source<'a>,
    tokens: &'a [Token],
    pos: usize,
    /// While a node's children are parsed: the levels above them. When a parse function returns:
    /// the height of the deepest path through the tree it built, counted from the level it was
    /// entered at, in the same units. A node that wraps what it already parsed, as an operator
    /// or a `.` does in a chain, adds a level to that height, so no way of building a tree gets
    /// past `MAX_DEPTH`. The height also counts the links and `else if`s on its path. An error
    /// ends the parse.
    depth: Height,
    /// The lexer's diagnostics, in source order.
    lexer: &'a [Diagnostic],
    /// Where each comment starts.
    comments: &'a [usize],
    /// Where each line break is.
    breaks: &'a [usize],
    /// The error of each bad format spec and each range written with `:` so far that no broken
    /// item took. The parser went on, so no other error follows them.
    recovered: Vec<Diagnostic>,
    /// Whether the parser reads the head of a `for`, `if`, `while` or `match`, outside any
    /// bracket or block in it. There, a `{` after `..` opens the body, not the end of the range.
    header: bool,
    /// Where the pattern being read stands. It says which tokens can end the pattern
    /// (`pattern_ends`). Each pattern sets it again, so a pattern in an interpolation does not
    /// take the place of the pattern around it.
    pattern_place: PatternPlace,
    /// The lexer's warnings, which `program` reports whether or not their item is broken.
    warnings: Vec<Diagnostic>,
    /// The loop labels open around the next token, innermost last: from each one's `name:` to the
    /// end of its loop's body.
    labels: Vec<Ident>,
    /// The loops open around the next token, innermost last, each with the part that holds it.
    /// The `( )` help of `jump_takes_block` keeps a `break` only in the body of its loop.
    loops: Vec<LoopPart>,
    /// Whether this is the prelude, which may declare a variant `None`.
    prelude: bool,
    /// Whether the parser reads the body of a struct, whose methods take a `self` that cannot
    /// change.
    struct_self: bool,
    /// `tokens`, or once a type-argument list takes the `>` of a `>=` (a split), the tokens before
    /// it. `peek` reads it with the bounds check it needs anyway, so only a split pays for one.
    window: &'a [Token],
    /// The `=` left of each `>=`, built at the first split.
    halves: &'a OnceCell<Vec<Token>>,
    /// The index of the `>=` whose `=` half the last `bump_past_window` took, if the token just
    /// consumed was that half.
    halved: Option<usize>,
    /// While a `match` reads its scrutinee: the index of its first token. A jump at the end of
    /// the scrutinee that takes the body's `{` as its value reports that (`jump_value`).
    scrutinee: Option<usize>,
}

/// An open loop, and whether the parser is in its header or in its body.
struct LoopPart {
    /// The loop's label, when it has one.
    label: Option<Symbol>,
    /// Whether the next token is in the loop's header (`while`, `while let` and `for`; a `loop`
    /// has none). A `break` there is E0082.
    header: bool,
    /// Whether a `break` has left this loop. A parse that is read again can leave it `true` after
    /// the `break` is gone, and `loop_expr` then keeps the rank that it had before.
    left: bool,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> &'a Token {
        match self.window.get(self.pos) {
            Some(token) => token,
            None => self.past_window(),
        }
    }

    /// The token at `pos`, which is past `window`: the `=` left of the split `>=` there, or, once
    /// the parser has moved past that, the token itself. `Eof` is last and never consumed, so
    /// `pos` stays in range.
    #[cold]
    fn past_window(&self) -> &'a Token {
        if !self.split() {
            return &self.tokens[self.pos];
        }
        let halves = self.halves.get_or_init(|| {
            let ge = self.tokens.iter().filter(|t| t.kind == TokenKind::Ge);
            ge.map(|t| Token {
                kind: TokenKind::Eq,
                span: Span::new(t.span.start() + 1, t.span.end()),
            })
            .collect()
        });
        let start = self.tokens[self.pos].span.start() + 1;
        let half = halves
            .binary_search_by_key(&start, |t| t.span.start())
            .expect("a split token should be a `>=`, which `halves` holds");
        &halves[half]
    }

    /// Whether `pos` is at a split `>=`.
    fn split(&self) -> bool {
        self.pos == self.window.len()
    }

    fn bump(&mut self) -> &'a Token {
        let Some(token) = self.window.get(self.pos) else {
            return self.bump_past_window();
        };
        if token.kind != TokenKind::Eof {
            self.pos += 1;
        }
        token
    }

    /// `bump` past `window`, which then covers every token again.
    #[cold]
    fn bump_past_window(&mut self) -> &'a Token {
        let split = self.split();
        let token = self.past_window();
        self.window = self.tokens;
        if token.kind != TokenKind::Eof {
            self.halved = split.then_some(self.pos);
            self.pos += 1;
        }
        token
    }

    fn at(&self, kind: &TokenKind) -> bool {
        // Most tokens asked about are of another kind, which the discriminant settles inline.
        let next = &self.peek().kind;
        mem::discriminant(next) == mem::discriminant(kind) && next == kind
    }

    /// Consumes the next token if it is `kind`.
    fn eat(&mut self, kind: &TokenKind) -> Option<&'a Token> {
        self.at(kind).then(|| self.bump())
    }

    /// Sets `self.depth`, or reports that the tree is too deep. The report names what made the
    /// path deep. For an `else if` chain, it points at the last `else if` of the longest run on
    /// the path, not at the token in its branch where the depth passed the limit. When `if` is on
    /// the line after `else`, it points at the `else` alone, so that the span stays on one line.
    fn reach(&mut self, height: Height) -> Result<(), Diagnostic> {
        self.depth = height;
        if height.depth > MAX_DEPTH {
            let form = height.form();
            let at = match height.longest {
                Some((_, else_if)) if matches!(form, NestingForm::ElseIf) => {
                    self.same_line_start(else_if, "else".len())
                }
                _ => self.peek().span,
            };
            return Err(DiagnosticKind::NestingTooDeep {
                at,
                limit: MAX_DEPTH,
                form,
            }
            .into());
        }
        Ok(())
    }

    /// `span`, or its first `len` bytes when a line break is inside `span`.
    fn same_line_start(&self, span: Span, len: usize) -> Span {
        // `breaks` is sorted, so the lookup is a binary search.
        let at = self.breaks.partition_point(|&b| b < span.start());
        if self.breaks.get(at).is_some_and(|&b| b < span.end()) {
            Span::new(span.start(), span.start() + len)
        } else {
            span
        }
    }

    /// Enters one more level of nesting for the children that follow.
    fn descend(&mut self) -> Result<(), Diagnostic> {
        self.reach(self.depth.deeper())
    }

    /// Parses one child of a node whose children start at depth `base`, and raises `reach`, the
    /// height of the node so far, to the child's. Every parse function follows the same pattern:
    /// reset `self.depth` to the base before each child, and keep the maximum in `reach`.
    fn child<T>(
        &mut self,
        base: Height,
        reach: &mut Height,
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
    /// A stray `var` or reserved word always gets its own diagnostic, wherever it appears, and so
    /// does a lone `&`: only a bound reads one, so anywhere else it is the unexpected character.
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
            TokenKind::Amp => DiagnosticKind::UnexpectedCharacter {
                c: '&',
                at: found.span,
            },
            TokenKind::Int(_) => DiagnosticKind::Expected {
                what: what.to_string(),
                found: format!("`{}`", self.source.text(found.span)),
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

    // Inlined: declarations, types, labels and members all read their names through it, and LLVM
    // stopped inlining it once its callers grew.
    #[inline]
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
        let mut optional = 0;
        let mut grouped = false;
        let mut inner_bare = None;
        let (mut kind, mut span) = if self.eat(&TokenKind::LBracket).is_some() {
            let element = self.parse_type()?;
            let close = self.expect(&TokenKind::RBracket)?;
            (TypeKind::Array(Box::new(element)), start.to(close.span))
        } else if self.eat(&TokenKind::LParen).is_some() {
            // `( type )` is the type, so `(any Observer)?` is an optional interface value.
            let inner = self.parse_type()?;
            let close = self.expect(&TokenKind::RParen)?;
            optional = inner.optional;
            grouped = true;
            if matches!(inner.kind, TypeKind::Name(_) | TypeKind::Applied(_)) {
                // The checker finds a name at the start of its span: `(Shape)` keeps `Shape`'s.
                inner_bare = Some(inner.bare);
                (inner.kind, inner.span)
            } else {
                (inner.kind, start.to(close.span))
            }
        } else if let Some(token) = self.eat(&TokenKind::SelfType) {
            (TypeKind::SelfType, token.span)
        } else if self.at_any() {
            return self.parse_any();
        } else {
            self.named_type()?
        };
        // `.Name` after a name, `Self` or a type with arguments is an associated type, one level
        // deeper each. A `.` and a name are always a member here: a type is never followed by a
        // field access.
        while !grouped
            && !matches!(kind, TypeKind::Array(_))
            && self.at(&TokenKind::Dot)
            && let TokenKind::Ident(member) = self.tokens[self.pos + 1].kind
        {
            self.descend()?;
            self.bump();
            let name = Ident {
                name: member,
                span: self.bump().span,
            };
            let base = TypeExpr {
                kind,
                optional: 0,
                span,
                bare: span,
            };
            span = span.to(name.span);
            kind = TypeKind::Member {
                base: Box::new(base),
                name,
            };
        }
        // Each `?` is one layer and one level of depth; `??` is one token of two.
        let bare = inner_bare.unwrap_or(span);
        loop {
            let layers = match self.peek().kind {
                TokenKind::Question => 1,
                TokenKind::QuestionQuestion => 2,
                _ => break,
            };
            for _ in 0..layers {
                self.descend()?;
            }
            optional += layers;
            span = span.to(self.bump().span);
        }
        Ok(TypeExpr {
            kind,
            optional,
            span,
            bare,
        })
    }

    /// A name, and its type arguments if `<` follows.
    fn named_type(&mut self) -> Result<(TypeKind, Span), Diagnostic> {
        let ident = self.ident("a type")?;
        if !self.at(&TokenKind::Lt) {
            return Ok((TypeKind::Name(ident.name), ident.span));
        }
        let (args, angles) = self.type_args()?;
        Ok((
            TypeKind::Applied(Box::new(Applied {
                name: ident.name,
                args,
                angles,
            })),
            Span::new(ident.span.start(), self.consumed_end()),
        ))
    }

    /// Whether `any` and a name come next: `any` before a name is the keyword, and anywhere else
    /// it is a type's name.
    fn at_any(&self) -> bool {
        matches!(self.peek().kind, TokenKind::Ident(name) if name.as_str() == "any")
            && matches!(self.tokens[self.pos + 1].kind, TokenKind::Ident(_))
    }

    /// `any I` or `any I<A>`, at the `any`. A `?` or a `&` after it is `AnyForm`: `any` binds
    /// tighter than `?`, and takes one interface.
    fn parse_any(&mut self) -> Result<TypeExpr, Diagnostic> {
        let keyword = self.bump().span;
        self.descend()?;
        let (kind, name_span) = self.named_type()?;
        let interface = TypeExpr {
            kind,
            optional: 0,
            span: name_span,
            bare: name_span,
        };
        let span = keyword.to(name_span);
        if self.peek().kind == TokenKind::Amp {
            let amp = self.bump().span;
            let other = self.parse_type()?;
            return Err(DiagnosticKind::AnyForm {
                at: amp.to(other.span),
                form: AnyFormKind::Several {
                    first: self.source.type_text(name_span).into_owned(),
                    second: self.source.type_text(other.bare).into_owned(),
                },
            }
            .into());
        }
        // Each `?` after the interface is a layer, and a `??` token is two.
        let (mut layers, mut mark) = (0, span);
        while let Some(count) = match self.peek().kind {
            TokenKind::Question => Some(1),
            TokenKind::QuestionQuestion => Some(2),
            _ => None,
        } {
            layers += count;
            mark = self.bump().span;
        }
        if layers == 0 {
            return Ok(TypeExpr {
                kind: TypeKind::Any(Box::new(interface)),
                optional: 0,
                span,
                bare: span,
            });
        }
        Err(DiagnosticKind::AnyForm {
            at: span.to(mark),
            form: AnyFormKind::Optional {
                written: self.source.type_text(span).into_owned(),
                bare: span,
                layers,
            },
        }
        .into())
    }

    /// `<` types `>` at the next token, with an optional trailing comma, and its span. The caller
    /// has seen the `<`, and for an expression, that `type_args_follow` holds.
    fn type_args(&mut self) -> Result<(Vec<TypeExpr>, Span), Diagnostic> {
        let open = self.bump().span;
        let base = self.depth;
        let mut reach = base;
        let mut args = Vec::new();
        loop {
            args.push(self.child(base, &mut reach, Self::parse_type)?);
            if self.eat(&TokenKind::Comma).is_none()
                || matches!(self.peek().kind, TokenKind::Gt | TokenKind::Ge)
            {
                break;
            }
        }
        self.close_angle()?;
        self.depth = reach;
        Ok((args, Span::new(open.start(), self.consumed_end())))
    }

    /// The `>` that closes a list. A `>=` closes it too, and then reads as the `=` it holds: in
    /// `let s: Stack<i64>= x`, the `>` is the list's.
    fn close_angle(&mut self) -> Result<(), Diagnostic> {
        if self.eat(&TokenKind::Gt).is_some() {
            return Ok(());
        }
        if self.at(&TokenKind::Ge) {
            self.window = &self.tokens[..self.pos];
            return Ok(());
        }
        Err(self.expected("`,` or `>`"))
    }

    /// Where the text that the parser consumed ends. A `>=` that `close_angle` split counts only
    /// as the `>` it gave to the list.
    fn consumed_end(&self) -> usize {
        if self.split() {
            // What is left of the `>=` starts where the `>` ends.
            self.peek().span.start()
        } else {
            self.last_span().end()
        }
    }

    /// Whether the `<` at the next token starts a type-argument list in an expression: the tokens
    /// from it form one by the type grammar, and a `(` or `.` follows its `>`. Anything else is a
    /// comparison. The scan builds nothing, so a failed scan leaves no diagnostic behind.
    fn type_args_follow(&self) -> bool {
        let mut at = self.pos;
        self.scan_list(&mut at, 0) && matches!(self.scanned(at), TokenKind::LParen | TokenKind::Dot)
    }

    /// The kind of the token at `at`, which the scan counts as visited.
    fn scanned(&self, at: usize) -> &'a TokenKind {
        #[cfg(test)]
        SCANNED.set(SCANNED.get() + 1);
        &self.tokens[at].kind
    }

    /// Scans a list from its `<` at `at` through its `>`, which `at` then follows. A list nested
    /// deeper than `MAX_DEPTH` is none (`scan_type` stops it), and that cutoff keeps a run of
    /// `a < a < a < …` linear: a name followed by `<` inside a list opens a nested one, so
    /// `MAX_DEPTH + 1` scans visit a token at most.
    fn scan_list(&self, at: &mut usize, depth: usize) -> bool {
        *at += 1;
        loop {
            if !self.scan_type(at, depth + 1) {
                return false;
            }
            match self.scanned(*at) {
                TokenKind::Comma => {
                    *at += 1;
                    if *self.scanned(*at) == TokenKind::Gt {
                        *at += 1;
                        return true;
                    }
                }
                TokenKind::Gt => {
                    *at += 1;
                    return true;
                }
                _ => return false,
            }
        }
    }

    /// Whether the tokens at `at` start with `any` and a name, as `at_any` asks of the next token.
    fn scans_any(&self, at: usize) -> bool {
        matches!(self.scanned(at), TokenKind::Ident(name) if name.as_str() == "any")
            && matches!(self.scanned(at + 1), TokenKind::Ident(_))
    }

    /// Scans a type: a name or `Self` and its list and `.Name`s, `any` and a name and its list,
    /// or `[type]`, then its `?` layers, each one level deep. A type in parentheses is one in an
    /// expression only when it starts with `any` or a `?` follows its `)`, since the other
    /// parentheses after a `<` are an expression's: `f(a < (b), c > (d))` is two comparisons.
    fn scan_type(&self, at: &mut usize, mut depth: usize) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        let mut members = 0;
        let mut interface = false;
        match self.scanned(*at) {
            TokenKind::LBracket => {
                *at += 1;
                if !self.scan_type(at, depth + 1) || *self.scanned(*at) != TokenKind::RBracket {
                    return false;
                }
                *at += 1;
            }
            TokenKind::LParen => {
                let any = self.scans_any(*at + 1);
                *at += 1;
                if !self.scan_type(at, depth + 1) || *self.scanned(*at) != TokenKind::RParen {
                    return false;
                }
                *at += 1;
                if !any
                    && !matches!(
                        self.scanned(*at),
                        TokenKind::Question | TokenKind::QuestionQuestion
                    )
                {
                    return false;
                }
            }
            TokenKind::Ident(_) | TokenKind::SelfType => {
                if self.scans_any(*at) {
                    // The interface is a level deeper, as `parse_any` counts it.
                    *at += 1;
                    depth += 1;
                    interface = true;
                }
                *at += 1;
                if *self.scanned(*at) == TokenKind::Lt && !self.scan_list(at, depth) {
                    return false;
                }
                // Each `.Name` is a level deeper, as `parse_type` counts it.
                while !interface
                    && *self.scanned(*at) == TokenKind::Dot
                    && matches!(self.scanned(*at + 1), TokenKind::Ident(_))
                {
                    members += 1;
                    if depth + members > MAX_DEPTH {
                        return false;
                    }
                    *at += 2;
                }
            }
            _ => return false,
        }
        let mut layers = 0;
        loop {
            layers += match self.scanned(*at) {
                TokenKind::Question => 1,
                TokenKind::QuestionQuestion => 2,
                _ => return depth + members + layers <= MAX_DEPTH,
            };
            *at += 1;
        }
    }

    /// `<T, U: Shape & Same>` after the name of a function, class, enum or interface, or after
    /// `extend`, with an optional trailing comma; empty when no `<` follows.
    fn type_params(&mut self) -> Result<Vec<TypeParam>, Diagnostic> {
        let mut params = Vec::new();
        if self.eat(&TokenKind::Lt).is_none() {
            return Ok(params);
        }
        loop {
            let name = self.name("a type parameter")?;
            let bounds = self.bounds()?;
            params.push(TypeParam { name, bounds });
            if self.eat(&TokenKind::Comma).is_none() || self.at(&TokenKind::Gt) {
                break;
            }
        }
        if self.eat(&TokenKind::Gt).is_none() {
            return Err(self.expected("`,` or `>`"));
        }
        Ok(params)
    }

    /// `: A & B<T>` after a type parameter or an associated type: the interfaces it must meet;
    /// empty when no `:` follows.
    fn bounds(&mut self) -> Result<Vec<TypeExpr>, Diagnostic> {
        let mut bounds = Vec::new();
        if self.eat(&TokenKind::Colon).is_none() {
            return Ok(bounds);
        }
        loop {
            self.depth = Height::default();
            bounds.push(self.parse_type()?);
            if self.eat(&TokenKind::Amp).is_none() {
                return Ok(bounds);
            }
        }
    }

    /// `: A, B<T>` after the name of a class, enum or interface: the interfaces it lists; empty
    /// when no `:` follows.
    fn interface_list(&mut self) -> Result<Vec<TypeExpr>, Diagnostic> {
        let mut listed = Vec::new();
        if self.eat(&TokenKind::Colon).is_none() {
            return Ok(listed);
        }
        loop {
            self.depth = Height::default();
            listed.push(self.parse_type()?);
            if self.eat(&TokenKind::Comma).is_none() {
                return Ok(listed);
            }
        }
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
            self.depth = Height::default();
            let start = self.pos;
            let tokens = self.tokens;
            let previous = tokens[..start].last();
            let header = self.header(start);
            // An `extend` block has no name of its own: its target may be no name at all, and
            // the block is intact.
            let extends = self.tokens[start].kind == TokenKind::Extend;
            // An item without a name would be lost, so the checker does not run; it runs on to
            // the next item, so that a foreign function and its body make one error.
            let end = if header.is_some() || extends {
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
            let parsed = self.item(end);
            match parsed {
                Ok((item, errors)) => {
                    debug_assert!(
                        header.is_some() || extends,
                        "an unnamed item should hold an error"
                    );
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
        diagnostics.append(&mut self.recovered);
        diagnostics.append(&mut self.warnings);
        // An unnamed item starts where its line does, inside the item before, so items may overlap.
        debug_assert!(rising(&covered), "the covered spans should rise");
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
                // The lexer ends a line with an empty `Newline`; a `;` gives a `Newline` with text.
                newlines: self
                    .tokens
                    .iter()
                    .filter(|t| t.is_line_break())
                    .map(|t| t.span.start())
                    .collect(),
            },
            diagnostics,
            checkable,
        }
    }

    /// The item that starts here and runs to `end`, and the errors of its methods that it
    /// survives, or its error.
    fn item(&mut self, end: usize) -> Result<(Item, Vec<Diagnostic>), Diagnostic> {
        match self.peek().kind {
            TokenKind::Fn => self.checked(end, |p| {
                Ok((Item::Function(p.function(Receiver::Free)?), Vec::new()))
            }),
            TokenKind::Class => self.with_methods(end, |p, errors, methods| {
                p.class_body(errors, methods).map(Item::Class)
            }),
            TokenKind::Struct => self.with_methods(end, |p, errors, methods| {
                p.struct_body(errors, methods).map(Item::Struct)
            }),
            TokenKind::Enum => self.with_methods(end, |p, errors, methods| {
                p.enum_body(errors, methods).map(Item::Enum)
            }),
            TokenKind::Interface => self.with_methods(end, |p, errors, methods| {
                p.interface_body(errors, methods).map(Item::Interface)
            }),
            TokenKind::Extend => self.with_methods(end, |p, errors, methods| {
                p.extend_body(errors, methods).map(Item::Extend)
            }),
            TokenKind::Const => {
                self.checked(end, |p| Ok((Item::Const(p.constant(end)?), Vec::new())))
            }
            TokenKind::Type => self.checked(end, |p| Ok((Item::Alias(p.alias(end)?), Vec::new()))),
            _ => self.checked(end, |p| {
                Err(p.expected(
                    "`fn`, `class`, `struct`, `enum`, `interface`, `extend`, `const` or `type`",
                ))
            }),
        }
    }

    /// The name and kind of the item that starts at `start`, when its header is intact: `fn`,
    /// `class`, `enum`, `interface` or `const`, then a name, with no lexer diagnostic before the
    /// token after the name. `fn to$tal` lexes as `fn to tal`, so its name is not `to`. An
    /// `extend` block is named by its target: the name after `extend` and its `<...>`, if any.
    fn header(&self, start: usize) -> Option<Broken> {
        let kind = match self.tokens[start].kind {
            TokenKind::Fn => BrokenKind::Function,
            TokenKind::Class => BrokenKind::Class,
            TokenKind::Struct => BrokenKind::Struct,
            TokenKind::Enum => BrokenKind::Enum,
            TokenKind::Const => BrokenKind::Constant,
            TokenKind::Type => BrokenKind::Alias,
            TokenKind::Interface => BrokenKind::Interface,
            TokenKind::Extend => BrokenKind::Extend,
            _ => return None,
        };
        let at = if kind == BrokenKind::Extend {
            self.extend_target(start)?
        } else {
            start + 1
        };
        // Neither a keyword nor a name is `Eof`, so the two tokens after the keyword exist.
        let TokenKind::Ident(name) = &self.tokens[at].kind else {
            return None;
        };
        let header = self.tokens[start].span.start()..self.tokens[at + 1].span.start();
        if self.lexer_in(header).next().is_some() {
            return None;
        }
        Some(Broken {
            name: Ident {
                name: *name,
                span: self.tokens[at].span,
            },
            kind,
        })
    }

    /// Where the target of the `extend` at `start` starts: after its type-parameter list, which
    /// runs to the `>` that matches the `<`. `None` when that `>` is not on the line, or the next
    /// item starts first: the scan never reads past `item_end`, so each item costs its own tokens.
    fn extend_target(&self, start: usize) -> Option<usize> {
        let mut at = start + 1;
        if self.tokens[at].kind != TokenKind::Lt {
            return Some(at);
        }
        let mut open = 0usize;
        loop {
            match self.tokens[at].kind {
                TokenKind::Lt => open += 1,
                TokenKind::Gt => {
                    open -= 1;
                    if open == 0 {
                        return Some(at + 1);
                    }
                }
                TokenKind::Eof | TokenKind::LBrace | TokenKind::Newline => return None,
                _ if self.starts_item_line(at) => return None,
                _ => {}
            }
            at += 1;
        }
    }

    /// The end, exclusive, of the item or member that starts at `start`: past the `}` that
    /// brings the depth back to where it started, or at a line break at that depth, which comes
    /// before any `{`. A closer below that depth (a class's `}`) or `Eof` ends it first. So does
    /// an `fn`, `class`, `const`, `enum`, `interface` or `extend` that starts a line at that
    /// depth, also after a token
    /// that cannot end a line. A constant is one line, as a statement is, so a `}` in its value
    /// does not end it.
    fn item_end(&self, start: usize) -> usize {
        let line = matches!(self.tokens[start].kind, TokenKind::Const | TokenKind::Type);
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
                _ if depth == 0 && i > start && self.starts_item_line(i) => return i,
                TokenKind::Eof => return i,
                _ => {}
            }
        }
        unreachable!("the tokens should end with `Eof`")
    }

    /// Whether token `i` is `fn`, `class`, `struct`, `const`, `type`, `enum`, `interface` or
    /// `extend` and the first token on its line. These words only start an item or a member. So
    /// one at the start of a line starts the next one. This is true also when a token that cannot
    /// end a line, such as `+`, leaves no `Newline` before it.
    fn starts_item_line(&self, i: usize) -> bool {
        let (before, after) = (self.tokens[i - 1].span.end(), self.tokens[i].span.start());
        matches!(
            self.tokens[i].kind,
            TokenKind::Fn
                | TokenKind::Class
                | TokenKind::Struct
                | TokenKind::Const
                | TokenKind::Type
                | TokenKind::Enum
                | TokenKind::Interface
                | TokenKind::Extend
        ) && self.breaks.partition_point(|&b| b < before)
            < self.breaks.partition_point(|&b| after > b)
    }

    /// The end, exclusive, of the unnamed item that starts at `start`: the next line whose first
    /// token is `fn`, `class`, `const`, `enum`, `interface` or `extend` outside brackets, or
    /// `Eof`.
    fn next_line_item(&self, start: usize) -> usize {
        let mut depth = 0usize;
        for (i, token) in self.tokens.iter().enumerate().skip(start) {
            match token.kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => depth -= 1,
                _ if i > start && depth == 0 && self.starts_item_line(i) => return i,
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
        let line = matches!(self.tokens[start].kind, TokenKind::Const | TokenKind::Type);
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
        let recovered = self.recovered.len();
        let parsed = parse(self);
        let lexed = self.lexer_in(bytes).next().cloned();
        if let (Ok(_), None) = (&parsed, &lexed) {
            return parsed;
        }
        let error = lexed
            .into_iter()
            .chain(self.recovered.drain(recovered..))
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
        let recovered = self.recovered.len();
        let mut errors = Vec::new();
        let mut methods = Vec::new();
        let parsed = body(self, &mut errors, &mut methods);
        debug_assert!(rising(&methods), "the method spans should rise");
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
                    .chain(outside)
                    .chain(self.recovered.drain(recovered..))
                    .chain(parsed.err())
                    .reduce(earlier)
                    .expect("a broken item should hold an error");
                Err(error)
            }
        }
    }

    /// The lines of a block through its last, up to its `}`, which stays: `line` parses the one
    /// that starts here and runs to the given end, and pushes what it holds. A method whose error
    /// follows its name is broken alone: its error goes to `errors`, its bytes to `methods`, and
    /// its name to the result. Any other error breaks the whole item.
    fn member_lines(
        &mut self,
        errors: &mut Vec<Diagnostic>,
        methods: &mut Vec<Range<usize>>,
        mut line: impl FnMut(&mut Self, usize) -> Result<(), Diagnostic>,
    ) -> Result<Vec<Ident>, Diagnostic> {
        let mut broken_methods = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::RBrace) {
            self.depth = Height::default();
            let start = self.pos;
            let end = self.item_end(start);
            if let Err(error) = line(self, end) {
                let Some(Broken {
                    name,
                    kind: BrokenKind::Function,
                }) = self.header(start)
                else {
                    return Err(error);
                };
                methods.push(self.bytes(start, end));
                broken_methods.push(name);
                errors.push(error);
            }
            self.skip_newlines();
        }
        Ok(broken_methods)
    }

    /// The class itself, adding each broken method's error to `errors` and its bytes to
    /// `methods`.
    fn class_body(
        &mut self,
        errors: &mut Vec<Diagnostic>,
        methods: &mut Vec<Range<usize>>,
    ) -> Result<Class, Diagnostic> {
        let Struct {
            name,
            type_params,
            conforms,
            assoc,
            members,
            broken_methods,
            ..
        } = self.type_body(errors, methods, &TokenKind::Class, "a class name")?;
        Ok(Class {
            name,
            type_params,
            conforms,
            assoc,
            members,
            broken_methods,
        })
    }

    /// The struct itself, as `class_body` reads a class. A `mut` field is E0175 (`member`).
    fn struct_body(
        &mut self,
        errors: &mut Vec<Diagnostic>,
        methods: &mut Vec<Range<usize>>,
    ) -> Result<Struct, Diagnostic> {
        self.type_body(errors, methods, &TokenKind::Struct, "a struct name")
    }

    /// The body of a class or a struct, which start with `word` and then a name that `what`
    /// describes. The two share it, so it gives an `ast::Struct`, the one with a keyword: a class
    /// drops the keyword.
    fn type_body(
        &mut self,
        errors: &mut Vec<Diagnostic>,
        methods: &mut Vec<Range<usize>>,
        word: &TokenKind,
        what: &str,
    ) -> Result<Struct, Diagnostic> {
        let keyword = self.expect(word)?.span;
        let name = self.name(what)?;
        let type_params = self.type_params()?;
        let conforms = self.interface_list()?;
        self.expect(&TokenKind::LBrace)?;
        let mut members = Vec::new();
        let mut assoc = Vec::new();
        // Only a type that lists an interface may choose an associated type.
        let chooses = !conforms.is_empty();
        let owner = if *word == TokenKind::Struct {
            Owner::Struct
        } else {
            Owner::Class
        };
        let outer = mem::replace(&mut self.struct_self, matches!(owner, Owner::Struct));
        let broken_methods = self.member_lines(errors, methods, |p, end| {
            match p.checked(end, |p| p.class_line(chooses, &owner))? {
                ClassLine::Member(member) => members.push(member),
                ClassLine::Assoc(choice) => assoc.push(choice),
            }
            Ok(())
        });
        self.struct_self = outer;
        let broken_methods = broken_methods?;
        self.bump();
        Ok(Struct {
            keyword,
            name,
            type_params,
            conforms,
            assoc,
            members,
            broken_methods,
        })
    }

    /// A member of a class or a struct, whose methods `owner` owns, and the end of its line. When
    /// `chooses`, it can also be the choice of an associated type.
    fn class_line(&mut self, chooses: bool, owner: &Owner) -> Result<ClassLine, Diagnostic> {
        if chooses && self.at(&TokenKind::Type) {
            let choice = self.assoc_choice()?;
            self.line_end()?;
            return Ok(ClassLine::Assoc(choice));
        }
        self.member_line(owner).map(ClassLine::Member)
    }

    /// `type Item = i64`, in a class, an enum or an `extend` block.
    fn assoc_choice(&mut self) -> Result<AssocChoice, Diagnostic> {
        self.bump();
        let name = self.name("an associated type name")?;
        self.expect(&TokenKind::Eq)?;
        self.depth = Height::default();
        Ok(AssocChoice {
            name,
            ty: self.parse_type()?,
        })
    }

    /// `type Item` or `type Item: Same & Named`, in an interface.
    fn assoc_type(&mut self) -> Result<AssocType, Diagnostic> {
        self.bump();
        let name = self.name("an associated type name")?;
        Ok(AssocType {
            name,
            bounds: self.bounds()?,
        })
    }

    /// The end of a member's line: a line break, a `;` that stands in for one, or the closing `}`.
    fn line_end(&self) -> Result<(), Diagnostic> {
        if !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Newline) {
            return Err(self.end_of_line());
        }
        Ok(())
    }

    /// A member whose methods `owner` owns, and the end of its line.
    fn member_line(&mut self, owner: &Owner) -> Result<Member, Diagnostic> {
        let member = self.member(owner)?;
        if matches!(member, Member::Field(_)) && self.at(&TokenKind::Comma) {
            return Err(self.member_comma(diagnostic::Member::Field));
        }
        self.line_end()?;
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
        ) || self
            .source
            .slice(comma.end()..next.span.start())
            .contains('\n');
        let replacement = if separated { "" } else { ";" }.to_string();
        DiagnosticKind::FieldComma {
            at: comma,
            member,
            replacement,
        }
        .into()
    }

    fn member(&mut self, owner: &Owner) -> Result<Member, Diagnostic> {
        match self.peek().kind {
            TokenKind::Ident(_) | TokenKind::Underscore | TokenKind::Mut => {
                let word = self.eat(&TokenKind::Mut).map(|token| token.span);
                let name = self.name("a field name")?;
                // A struct field with `mut` parses with `mutable` false, so the checker sees a
                // valid struct, and the parser goes on after the error.
                let mutable = match (word, owner) {
                    (Some(keyword), Owner::Struct) => {
                        let remove = Span::new(
                            keyword.start(),
                            self.removal_end(keyword.end(), name.span.start()),
                        );
                        self.recovered
                            .push(DiagnosticKind::StructFieldMut { keyword, remove }.into());
                        false
                    }
                    (word, _) => word.is_some(),
                };
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
            TokenKind::Fn => Ok(Member::Method(self.method(Receiver::Of(owner))?)),
            _ => Err(self.expected("a field or `fn`")),
        }
    }

    /// A method of `receiver`, which must take `self`.
    fn method(&mut self, receiver: Receiver<'_>) -> Result<Function, Diagnostic> {
        let method = self.function(receiver)?;
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
                    name: self.source.text(next.span).to_string(),
                    remove: Span::new(
                        class.start(),
                        self.removal_end(class.end(), next.span.start()),
                    ),
                },
                at: class,
            }
            .into());
        }
        let name = self.name("an enum name")?;
        let type_params = self.type_params()?;
        let conforms = self.interface_list()?;
        let owner = Owner::Enum(name.name.to_string());
        self.expect(&TokenKind::LBrace)?;
        let mut variants = Vec::new();
        let mut assoc = Vec::new();
        let mut fns = Vec::new();
        // The `fn` of the first method, broken or not.
        let mut first_method = None;
        // Only an enum that lists an interface may choose an associated type.
        let chooses = !conforms.is_empty();
        let broken_methods = self.member_lines(errors, methods, |p, end| {
            if first_method.is_none() && p.at(&TokenKind::Fn) {
                first_method = Some(p.peek().span);
            }
            match p.checked(end, |p| p.enum_line(&owner, first_method, chooses))? {
                EnumMember::Variant(variant) => variants.push(variant),
                EnumMember::Method(method) => fns.push(method),
                EnumMember::Assoc(choice) => assoc.push(choice),
            }
            Ok(())
        })?;
        self.bump();
        Ok(Enum {
            name,
            type_params,
            conforms,
            assoc,
            variants,
            methods: fns,
            broken_methods,
        })
    }

    /// A variant or a method of `owner` or, when `chooses`, the choice of an associated type, and
    /// the end of its line. `first_method` is the `fn` of the first method before this line.
    fn enum_line(
        &mut self,
        owner: &Owner,
        first_method: Option<Span>,
        chooses: bool,
    ) -> Result<EnumMember, Diagnostic> {
        let member = if self.at(&TokenKind::Fn) {
            EnumMember::Method(self.method(Receiver::Of(owner))?)
        } else if chooses && self.at(&TokenKind::Type) {
            EnumMember::Assoc(self.assoc_choice()?)
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
        self.line_end()?;
        Ok(member)
    }

    /// `Name`, or `Name(payload)`. Only the prelude names a variant `None`.
    fn variant(&mut self) -> Result<Variant, Diagnostic> {
        let name = if self.prelude && self.at(&TokenKind::NoneKw) {
            Ident {
                name: Symbol::NONE,
                span: self.bump().span,
            }
        } else if self.at(&TokenKind::NoneKw) {
            return Err(DiagnosticKind::BuiltInName {
                name: "None".to_string(),
                at: self.peek().span,
                kind: BuiltIn::Keyword,
            }
            .into());
        } else {
            self.ident("a variant or `fn`")?
        };
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
            self.depth = Height::default();
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
            recovered: Vec::new(),
            header: false,
            pattern_place: PatternPlace::Arm,
            warnings: Vec::new(),
            labels: Vec::new(),
            loops: Vec::new(),
            prelude: self.prelude,
            struct_self: self.struct_self,
            window: self.tokens,
            halves: self.halves,
            halved: None,
            scrutinee: None,
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
                        self.removal_end(
                            self.peek().span.end(),
                            self.tokens[self.pos + 1].span.start(),
                        ),
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
        self.depth = Height::default();
        self.expect(&TokenKind::Eq)?;
        let value = self.expr()?;
        if self.pos != end {
            return Err(self.end_of_line());
        }
        Ok(Const { name, ty, value })
    }

    /// `type Name<A, B> = T`, which ends the item at `end`.
    fn alias(&mut self, end: usize) -> Result<AliasDecl, Diagnostic> {
        let keyword = self.bump().span;
        let name = self.name("a type alias name")?;
        let mut params = Vec::new();
        if self.eat(&TokenKind::Lt).is_some() {
            loop {
                params.push(self.name("a type parameter")?);
                if self.eat(&TokenKind::Comma).is_none() || self.at(&TokenKind::Gt) {
                    break;
                }
            }
            if self.eat(&TokenKind::Gt).is_none() {
                return Err(self.expected("`,` or `>`"));
            }
        }
        self.expect(&TokenKind::Eq)?;
        self.depth = Height::default();
        let ty = self.parse_type()?;
        if self.pos != end {
            return Err(self.end_of_line());
        }
        Ok(AliasDecl {
            name,
            params,
            span: keyword.to(ty.span),
            ty,
        })
    }

    /// `receiver` says where the function stands, and so whether `self` may be its first
    /// parameter.
    fn function(&mut self, receiver: Receiver<'_>) -> Result<Function, Diagnostic> {
        let Signature {
            fn_span,
            name,
            type_params,
            angles,
            self_param,
            mut_self,
            params,
            ret,
        } = self.signature(receiver)?;
        self.depth = Height::default();
        let body = self.block()?;
        Ok(Function {
            name,
            type_params,
            angles,
            self_param,
            mut_self,
            params,
            ret,
            span: fn_span.to(body.span),
            body,
        })
    }

    /// A function's signature, from its `fn` through its return type.
    fn signature(&mut self, kind: Receiver<'_>) -> Result<Signature, Diagnostic> {
        let fn_token = self.expect(&TokenKind::Fn)?;
        let name = self.name("a function name")?;
        let open = self.peek().span;
        let type_params = self.type_params()?;
        let angles = if type_params.is_empty() {
            name.span.end_point()
        } else {
            open.to(self.last_span())
        };
        self.expect(&TokenKind::LParen)?;
        let mut self_param = None;
        let mut mut_self = None;
        let mut params = Vec::new();
        if !self.at(&TokenKind::RParen) {
            loop {
                let first = self_param.is_none() && params.is_empty();
                let receiver = if first { kind } else { Receiver::Free };
                let mut mutable = false;
                if self.at(&TokenKind::Mut) {
                    // `mut` is never `Eof`, so the token after it exists.
                    match (&self.tokens[self.pos + 1].kind, receiver) {
                        (TokenKind::SelfKw, Receiver::Of(owner)) => {
                            return Err(self.mut_self_error(owner.clone()));
                        }
                        (TokenKind::SelfKw, Receiver::Extension) => {
                            mut_self = Some(MutSelf {
                                keyword: self.bump().span,
                                receiver: self.peek().span,
                                annotation: None,
                            });
                        }
                        (TokenKind::SelfKw, Receiver::Free) => {
                            self.bump();
                        }
                        (TokenKind::Ident(_), _) => {
                            self.bump();
                            mutable = true;
                        }
                        _ => {}
                    }
                }
                if self.at(&TokenKind::SelfKw) {
                    if matches!(receiver, Receiver::Free) {
                        return Err(DiagnosticKind::SelfNotFirst {
                            at: self.peek().span,
                        }
                        .into());
                    }
                    self_param = Some(self.bump().span);
                    // The annotation of a `mut self` is part of the error the checker reports.
                    if let Some(mut_self) = &mut mut_self
                        && let Some(colon) = self.eat(&TokenKind::Colon)
                    {
                        self.parse_type()?;
                        mut_self.annotation =
                            Some(Span::new(colon.span.start(), self.consumed_end()));
                    }
                    if self.eat(&TokenKind::Comma).is_none() {
                        break;
                    }
                    continue;
                }
                let name = self.name("a parameter name")?;
                self.expect(&TokenKind::Colon)?;
                let ty = self.parse_type()?;
                self.depth = Height::default();
                params.push(Param { name, ty, mutable });
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
        Ok(Signature {
            fn_span: fn_token.span,
            name,
            type_params,
            angles,
            self_param,
            mut_self,
            params,
            ret,
        })
    }

    /// An interface: its inherited interfaces, associated types and requirements, and the errors
    /// of the requirements it breaks, as `class_body` does.
    fn interface_body(
        &mut self,
        errors: &mut Vec<Diagnostic>,
        methods: &mut Vec<Range<usize>>,
    ) -> Result<Interface, Diagnostic> {
        let keyword = self.bump().span;
        let name = self.name("an interface name")?;
        let type_params = self.type_params()?;
        let inherits = self.interface_list()?;
        self.expect(&TokenKind::LBrace)?;
        let mut assoc = Vec::new();
        let mut requirements = Vec::new();
        let broken_methods = self.member_lines(errors, methods, |p, end| {
            match p.checked(end, Self::interface_line)? {
                InterfaceLine::Requirement(requirement) => requirements.push(requirement),
                InterfaceLine::Assoc(ty) => assoc.push(ty),
            }
            Ok(())
        })?;
        let close = self.bump().span;
        Ok(Interface {
            name,
            type_params,
            inherits,
            assoc,
            methods: requirements,
            broken_methods,
            span: keyword.to(close),
        })
    }

    /// A requirement or an associated type of an interface, and the end of its line. A body after
    /// a requirement, and a field, are errors.
    fn interface_line(&mut self) -> Result<InterfaceLine, Diagnostic> {
        let line = match self.peek().kind {
            TokenKind::Fn => InterfaceLine::Requirement(self.requirement()?),
            TokenKind::Type => InterfaceLine::Assoc(self.assoc_type()?),
            TokenKind::Ident(_)
            | TokenKind::Underscore
            | TokenKind::Mut
            | TokenKind::Let
            | TokenKind::Var => {
                // A field that is itself wrong gets its own error.
                let start = self.peek().span;
                // Its token is no `fn`, so no method takes the owner.
                self.member(&Owner::Class)?;
                return Err(DiagnosticKind::InterfaceBody {
                    at: start.to(self.last_span()),
                    form: InterfaceBodyForm::Field,
                }
                .into());
            }
            _ => return Err(self.expected("`fn` or `type`")),
        };
        self.line_end()?;
        Ok(line)
    }

    /// A method signature of an interface, which must take `self` and has no body.
    fn requirement(&mut self) -> Result<Requirement, Diagnostic> {
        let signature = self.signature(Receiver::Of(&Owner::Interface))?;
        let Some(self_param) = signature.self_param else {
            return Err(DiagnosticKind::MethodNeedsSelf {
                at: signature.name.span,
            }
            .into());
        };
        if self.at(&TokenKind::LBrace) {
            return Err(DiagnosticKind::InterfaceBody {
                at: self.peek().span,
                form: InterfaceBodyForm::Body,
            }
            .into());
        }
        Ok(Requirement {
            name: signature.name,
            type_params: signature.type_params,
            angles: signature.angles,
            self_param,
            params: signature.params,
            ret: signature.ret,
            span: signature.fn_span.to(self.last_span()),
        })
    }

    /// An `extend` block: its target, the interface it makes the target meet, and its methods and
    /// associated types, with the errors of the methods it breaks, as `class_body` does.
    fn extend_body(
        &mut self,
        errors: &mut Vec<Diagnostic>,
        methods: &mut Vec<Range<usize>>,
    ) -> Result<Extend, Diagnostic> {
        let keyword = self.bump().span;
        let open = self.peek().span;
        let type_params = self.type_params()?;
        let angles = if type_params.is_empty() {
            keyword.end_point()
        } else {
            open.to(self.last_span())
        };
        self.depth = Height::default();
        let target = self.parse_type()?;
        let interface = match self.eat(&TokenKind::Colon) {
            Some(_) => {
                self.depth = Height::default();
                Some(self.parse_type()?)
            }
            None => None,
        };
        self.expect(&TokenKind::LBrace)?;
        let mut assoc = Vec::new();
        let mut fns = Vec::new();
        // Only a block that makes the target meet an interface may choose an associated type.
        let chooses = interface.is_some();
        let broken_methods = self.member_lines(errors, methods, |p, end| {
            match p.checked(end, |p| p.extend_line(chooses))? {
                ExtendLine::Method(method) => fns.push(method),
                ExtendLine::Assoc(choice) => assoc.push(choice),
            }
            Ok(())
        })?;
        let close = self.bump().span;
        Ok(Extend {
            keyword,
            type_params,
            angles,
            target,
            interface,
            assoc,
            methods: fns,
            broken_methods,
            span: keyword.to(close),
        })
    }

    /// A method or, when `chooses`, the choice of an associated type, in an `extend` block, and
    /// the end of its line.
    fn extend_line(&mut self, chooses: bool) -> Result<ExtendLine, Diagnostic> {
        let line = match self.peek().kind {
            TokenKind::Fn => ExtendLine::Method(self.method(Receiver::Extension)?),
            TokenKind::Type if chooses => ExtendLine::Assoc(self.assoc_choice()?),
            _ => return Err(self.expected("`fn`")),
        };
        self.line_end()?;
        Ok(line)
    }

    fn block(&mut self) -> Result<Block, Diagnostic> {
        self.headed(false, Self::block_inside)
    }

    /// `block`, with `header` already cleared.
    fn block_inside(&mut self) -> Result<Block, Diagnostic> {
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
        let open = (self.labels.len(), self.loops.len());
        let stmt = self.stmt_opening_labels();
        // A loop label and a loop part close with the loop's body, also when the loop fails to
        // parse.
        self.labels.truncate(open.0);
        self.loops.truncate(open.1);
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
            TokenKind::Let => self.let_stmt(true)?,
            TokenKind::Const => return Err(self.local_const()),
            TokenKind::While => {
                let keyword = self.bump().span;
                self.open_loop(label.as_ref(), true);
                if let Some((pattern, value)) = self.child(base, &mut reach, Self::let_binding)? {
                    self.header_body(&value)?;
                    self.enter_loop_body();
                    StmtKind::WhileLet {
                        label,
                        keyword,
                        pattern,
                        value,
                        body: self.child(base, &mut reach, Self::block)?,
                    }
                } else {
                    let cond = self.child(base, &mut reach, |p| p.headed(true, Self::expr))?;
                    self.header_body(&cond)?;
                    self.enter_loop_body();
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
                self.open_loop(label.as_ref(), true);
                let name = self.binding()?;
                self.expect(&TokenKind::In)?;
                // Another language's range has a fix only here: the checker takes this range.
                let source =
                    self.child(base, &mut reach, |p| p.headed(true, |p| p.range_expr(true)))?;
                self.header_body(&source)?;
                self.enter_loop_body();
                let source = match source {
                    Expr {
                        kind: ExprKind::Range(range),
                        ..
                    } => ForSource::Range(range),
                    array => ForSource::Array(array),
                };
                StmtKind::For {
                    label,
                    keyword,
                    name,
                    source,
                    body: self.child(base, &mut reach, Self::block)?,
                }
            }
            TokenKind::Return | TokenKind::Break | TokenKind::Continue => {
                StmtKind::Expr(self.jump(base, &mut reach)?)
            }
            TokenKind::LBrace => return Err(DiagnosticKind::BlockAsStatement { at: start }.into()),
            // A `mut` that starts a statement is most likely a declaration with no `let`, which
            // no safe fix can tell from an assignment.
            TokenKind::Mut => return Err(self.expected("an expression")),
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
                    remove: Span::new(at.start(), self.removal_end(at.end(), next.span.start())),
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

    /// The text of `span` on one line, for a diagnostic (`Source::line_text`). A line break that
    /// ends a statement keeps the text as written; those are the empty `Newline` tokens in `span`.
    /// The work is a binary search and a read of the tokens in `span`.
    fn line_text(&self, span: Span) -> Cow<'a, str> {
        let first = self
            .tokens
            .partition_point(|token| token.span.start() < span.start());
        let newlines: Vec<usize> = self.tokens[first..]
            .iter()
            .take_while(|token| token.span.start() < span.end())
            .filter(|token| token.is_line_break())
            .map(|token| token.span.start())
            .collect();
        self.source.line_text(span, self.comments, &newlines)
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
        self.line_text(start.to(end)).into_owned()
    }

    /// The `return`, `break` or `continue` at the next token, with its loop label and its value.
    /// The value takes the whole expression to its right, as `child` counts it from `base`.
    fn jump(&mut self, base: Height, reach: &mut Height) -> Result<Expr, Diagnostic> {
        let keyword_pos = self.pos;
        let keyword = self.bump();
        let kind = match keyword.kind {
            TokenKind::Return => ExprKind::Return {
                keyword: keyword.span,
                value: self.jump_value(base, reach, keyword_pos, None)?,
            },
            TokenKind::Break => {
                let label = self.jump_label(false);
                if let Some(error) = label.as_ref().and_then(|label| self.label_clash(label)) {
                    return Err(error);
                }
                let named = label.as_ref().map(|label| label.name);
                self.mark_left(named);
                ExprKind::Break {
                    label,
                    keyword: keyword.span,
                    value: self.jump_value(base, reach, keyword_pos, named)?,
                }
            }
            _ => ExprKind::Continue {
                label: self.jump_label(true),
            },
        };
        Ok(Expr {
            kind,
            span: keyword.span.to(self.last_span()),
        })
    }

    /// The value of a `return` or a `break` (its keyword is the token at `keyword_pos`, and
    /// `label` is the `break`'s loop label), unless the jump ends here. In the scrutinee of a
    /// `match`, a `{` after the jump is the body that the jump took, and when that body fails to
    /// parse as a block, as the arms do, the error is the jump's (`header_body` reports the same
    /// mistake when the block parses).
    fn jump_value(
        &mut self,
        base: Height,
        reach: &mut Height,
        keyword_pos: usize,
        label: Option<Symbol>,
    ) -> Result<Option<Box<Expr>>, Diagnostic> {
        if self.jump_ends() {
            return Ok(None);
        }
        let braced = self.at(&TokenKind::LBrace);
        let end = self.tokens[self.pos - 1].span.end();
        match self.child(base, reach, Self::expr) {
            Ok(value) => Ok(Some(Box::new(value))),
            Err(error) => match self.scrutinee {
                Some(first) if braced && self.brackets_closed(first, keyword_pos) => {
                    let keyword = &self.tokens[keyword_pos];
                    let name = if keyword.kind == TokenKind::Return {
                        "return"
                    } else {
                        "break"
                    };
                    let header = self.tokens[first].span.start();
                    Err(self.jump_takes_block(name, keyword.span, header, end, label))
                }
                _ => Err(error),
            },
        }
    }

    /// Whether the brackets of the tokens from `first` up to `last` all close in them.
    fn brackets_closed(&self, first: usize, last: usize) -> bool {
        let mut open = 0usize;
        for token in &self.tokens[first..last] {
            let Some(next) = nested(&token.kind, open) else {
                return false;
            };
            open = next;
        }
        open == 0
    }

    /// The error of the `return` or `break` `name` at `keyword`, whose value is the `{ ... }`
    /// that should be the body of the header that starts at byte `header`. The header through
    /// the jump, which ends at byte `end`, goes in `( )` to keep the block as the body. That is
    /// help only when the `break` (with loop label `label`) then stays in the body of its loop:
    /// in a loop header it is E0082, and with no loop it is E0038.
    fn jump_takes_block(
        &self,
        name: &'static str,
        keyword: Span,
        header: usize,
        end: usize,
        label: Option<Symbol>,
    ) -> Diagnostic {
        let grouped = (name == "return" || self.break_stays_in_body(label))
            .then(|| self.line_text(Span::new(header, end)).into_owned());
        DiagnosticKind::JumpTakesBlock {
            keyword: name,
            at: keyword,
            grouped,
        }
        .into()
    }

    /// Marks the loop that a `break` with loop label `label` leaves: the innermost one when the
    /// `break` has no label. It walks the open loops of the jump, so its cost is bounded by the
    /// nesting limit.
    fn mark_left(&mut self, label: Option<Symbol>) {
        let mut parts = self.loops.iter_mut().rev();
        let target = match label {
            None => parts.next(),
            Some(label) => parts.find(|part| part.label == Some(label)),
        };
        if let Some(part) = target {
            part.left = true;
        }
    }

    /// Whether a `break` with loop label `label` (the innermost loop, when it has none) leaves
    /// from the body of its loop, with no loop header on its way out. It walks the open loops of
    /// the jump, so its cost is bounded by the nesting limit.
    fn break_stays_in_body(&self, label: Option<Symbol>) -> bool {
        let Some(label) = label else {
            return self.loops.last().is_some_and(|part| !part.header);
        };
        for part in self.loops.iter().rev() {
            if part.label == Some(label) {
                return !part.header;
            }
            if part.header {
                return false;
            }
        }
        debug_assert!(
            false,
            "`jump_label` should find only the label of an open loop"
        );
        false
    }

    /// The error of a header (`header`, the condition, value or source of an `if`, `while`, `for`
    /// or `match`) whose body block is missing because a jump at its end took that block as its
    /// value: `if o ?? return {`. Nothing when a `{` follows, or the header ends in no such jump.
    fn header_body(&self, header: &Expr) -> Result<(), Diagnostic> {
        if self.at(&TokenKind::LBrace) {
            return Ok(());
        }
        let header_end = self.tokens[self.pos - 1].span.end();
        let Some(jump) = block_taking_jump(header, header_end) else {
            return Ok(());
        };
        let (name, keyword, label) = match &jump.kind {
            ExprKind::Return { keyword, .. } => ("return", *keyword, None),
            ExprKind::Break { label, keyword, .. } => ("break", *keyword, label.as_ref()),
            _ => unreachable!("`block_taking_jump` should find a jump"),
        };
        let end = label.map_or_else(|| keyword.end(), |label| label.span.end());
        let label = label.map(|label| label.name);
        Err(self.jump_takes_block(name, keyword, header.span.start(), end, label))
    }

    /// Opens a loop (with loop label `label`) in its header or, for a `loop`, in its body.
    fn open_loop(&mut self, label: Option<&Ident>, header: bool) {
        self.loops.push(LoopPart {
            label: label.map(|label| label.name),
            header,
            left: false,
        });
    }

    /// Moves the innermost loop, whose header has just been read, to its body.
    fn enter_loop_body(&mut self) {
        self.loops
            .last_mut()
            .expect("the loop's own part should still be open after its header")
            .header = false;
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

    /// Whether the next token ends a `return`, `break` or `continue`, which then takes no value:
    /// only a token that can start an expression continues one.
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
                | TokenKind::Else
                | TokenKind::Eof
                // The tokens that end a part of an interpolation: `}`, and a format spec.
                | TokenKind::StrMiddle(_)
                | TokenKind::StrTail(_)
                | TokenKind::FormatSpec(_)
        )
    }

    /// The `else` that starts a line, after `previous`, the statement before it. The `else` can
    /// move up to the `if` it follows when that has none, and a block or an `if` follows it.
    fn else_on_new_line(&self, previous: Option<&Stmt>) -> Diagnostic {
        // A pattern `let` without `else` never comes here: `let_pattern` reports the `else`.
        let value = previous.and_then(|stmt| match &stmt.kind {
            // A `let` with a type takes no `else`: `let_stmt` reports it, unless its value ends in
            // an `if` without `else`, which keeps the old E0006, with no join.
            StmtKind::Let { ty: None, init, .. } => Some(init),
            _ => None,
        });
        // The `else` of a `let` value that ends in an `if` without `else` belongs to that `if`.
        if value.is_some_and(|value| !ends_in_open_if(value)) {
            return self.let_else_on_new_line(self.pos);
        }
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
            safe: self.source.slice(gap.clone()).trim().is_empty(),
        });
        DiagnosticKind::ElseOnNewLine {
            at,
            of: ElseOf::If,
            join,
        }
        .into()
    }

    /// The `else` at token `else_at`, which starts a line after the value of a `let`. A `{`
    /// follows the `else` of a `let ... else`; the join puts it after the value. A value that
    /// ends in `}` would then read the `else` as its own (E0173), so the join is only a hint.
    fn let_else_on_new_line(&self, else_at: usize) -> Diagnostic {
        let join = self.else_gap(else_at).map(|gap| Join {
            gap,
            safe: self.source.slice(gap.start()..gap.end()).trim().is_empty(),
        });
        DiagnosticKind::ElseOnNewLine {
            at: self.tokens[else_at].span,
            of: ElseOf::Let,
            join,
        }
        .into()
    }

    /// The gap that a join of the `else` at token `else_at` replaces: from the end of the last
    /// token before it that is not a line break, to the `else`. There is none when no `{` follows
    /// the `else`, or when that token is a `}`: the `else` would then read as its own.
    fn else_gap(&self, else_at: usize) -> Option<Span> {
        let last = self.tokens[..else_at]
            .iter()
            .rfind(|t| t.kind != TokenKind::Newline);
        let end = last.map_or(0, |t| t.span.end());
        let ends_in_brace = last.is_some_and(|t| t.kind == TokenKind::RBrace);
        (self.tokens[else_at + 1].kind == TokenKind::LBrace && !ends_in_brace)
            .then(|| Span::new(end, self.tokens[else_at].span.start()))
    }

    /// The gap of the `else` at token `else_at` of a typed `let`, when a line break lies in it.
    /// The fix of the type joins such an `else` to the value, as the fixed source would report
    /// E0006 otherwise.
    fn type_fix_join(&self, else_at: usize) -> Option<Span> {
        self.else_gap(else_at)
            .filter(|gap| self.source.slice(gap.start()..gap.end()).contains('\n'))
    }

    /// Where the code before the next token ends: the `}` of the statement before it.
    fn last_code_end(&self) -> usize {
        self.tokens[..self.pos]
            .iter()
            .rfind(|t| t.kind != TokenKind::Newline)
            .map_or(0, |t| t.span.end())
    }

    /// The span of the token just consumed: the `=` half of a `>=` that closed a list, if that
    /// half was taken.
    fn last_span(&self) -> Span {
        let last = self.pos - 1;
        let span = self.tokens[last].span;
        if self.halved == Some(last) {
            Span::new(span.start() + 1, span.end())
        } else {
            span
        }
    }

    /// `let pattern = value`, after `if` or `while`; `None` when no `let` follows.
    fn let_binding(&mut self) -> Result<Option<(Pattern, Expr)>, Diagnostic> {
        if self.eat(&TokenKind::Let).is_none() {
            return Ok(None);
        }
        let base = self.depth;
        let mut reach = base;
        let pattern = self.child(base, &mut reach, |p| p.pattern_in(PatternPlace::Let))?;
        self.expect(&TokenKind::Eq)?;
        let value = self.child(base, &mut reach, |p| p.headed(true, Self::expr))?;
        self.depth = reach;
        Ok(Some((pattern, value)))
    }

    /// A pattern at `place`: see `pattern`. The place before it comes back after.
    fn pattern_in(&mut self, place: PatternPlace) -> Result<Pattern, Diagnostic> {
        let outer = mem::replace(&mut self.pattern_place, place);
        let pattern = self.pattern();
        self.pattern_place = outer;
        pattern
    }

    /// Whether a `|` follows, which joins the next alternative. In a sub-pattern, line breaks
    /// before the `|` are skipped: the lexer drops them in `( )` but keeps them in `{ }`, and
    /// no `|` can start a statement.
    fn pipe_follows(&mut self) -> bool {
        if self.pattern_place == PatternPlace::Sub && self.peek().is_line_break() {
            let at = self.after_line_breaks(self.pos);
            if self.tokens[at].kind == TokenKind::Pipe {
                while self.pos < at {
                    self.bump();
                }
            }
        }
        self.at(&TokenKind::Pipe)
    }

    /// A pattern: one or more `single`s joined by `|`.
    fn pattern(&mut self) -> Result<Pattern, Diagnostic> {
        self.descend()?;
        let base = self.depth;
        let mut reach = base;
        let first = self.child(base, &mut reach, Self::single)?;
        let pattern = if self.pipe_follows() {
            let mut span = first.span;
            let mut alternatives = vec![first];
            while self.pipe_follows() {
                self.bump();
                let next = self.child(base, &mut reach, Self::single)?;
                span = span.to(next.span);
                alternatives.push(next);
            }
            // The `Or` is the parent of its alternatives: one level more.
            self.reach(reach.deeper())?;
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

    /// An error for the type arguments at the next token, if a list follows that a `(` or `.` ends.
    /// A pattern takes none.
    fn no_type_args(&mut self) -> Result<(), Diagnostic> {
        if !self.at(&TokenKind::Lt) || !self.type_args_follow() {
            return Ok(());
        }
        let (_, at) = self.type_args()?;
        // The fix deletes the type arguments, so it would delete a comment inside them.
        let fix = self.removal_end(at.start(), at.end()) == at.end();
        Err(DiagnosticKind::PatternForm {
            form: ForeignPattern::TypeArgs { fix },
            at,
        }
        .into())
    }

    /// Whether a pattern can start with the token `kind`, which `single` reads.
    fn starts_pattern(kind: &TokenKind) -> bool {
        matches!(
            kind,
            TokenKind::Minus
                | TokenKind::Int(_)
                | TokenKind::Char(_)
                | TokenKind::Str(_)
                | TokenKind::StrHead(_)
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Underscore
                | TokenKind::NoneKw
                | TokenKind::Ident(_)
                | TokenKind::DotDot
                | TokenKind::DotDotEq
        )
    }

    /// One alternative of a pattern: `_`, `None`, a name, `Num(p, ...)`, `Expr.Num(p, ...)`,
    /// `Expr.Idle`, `Option.None`, a literal, or a range of two literals. A range with one bound
    /// is an error. The caller has counted its level.
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
                // `Option<i64>.Some(v)` or `Wrap<i64>(v)`: the matched value gives the types.
                self.no_type_args()?;
                let (qualifier, name) = if self.eat(&TokenKind::Dot).is_some() {
                    // `Option.None`, as in an expression; the checker rejects the `None` member
                    // of anything else.
                    let name = if self.at(&TokenKind::NoneKw) {
                        Ident {
                            name: Symbol::NONE,
                            span: self.bump().span,
                        }
                    } else {
                        self.ident("a variant name")?
                    };
                    (Some(first), name)
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
                                form: self.one_line_form(path, &fields),
                                // A comment between a label and its `:` would be deleted.
                                labels: labels
                                    .into_iter()
                                    .map(|(_, remove)| {
                                        (self.removal_end(remove.start(), remove.end())
                                            == remove.end())
                                        .then_some(remove)
                                    })
                                    .collect(),
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
                } else if self.at(&TokenKind::LBrace)
                    && let Some(error) = self.braced_pattern(path, base, &mut reach)?
                {
                    return Err(error);
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
            TokenKind::DotDot | TokenKind::DotDotEq => return Err(self.range_without_start()),
            _ => return Err(self.expected("a pattern")),
        };
        // A name or a literal can start a range; no other pattern is a bound.
        if matches!(self.peek().kind, TokenKind::DotDot | TokenKind::DotDotEq) {
            return Err(self.expected_at(token, "a literal"));
        }
        self.depth = reach;
        Ok(Pattern { kind, span })
    }

    /// Rust's `Rect { w, h }`, or with labels, `Rect { w: x, h: y }`, at the next token, after the
    /// pattern `path`: its error, when a pattern can end at the `}` that closes the `{`. Else the
    /// `{` is not part of a pattern: it stays the next token, `path` is the whole pattern, and the
    /// caller reports the `{`, before it reads anything inside the braces. A lexer error in the
    /// braces comes first, because an unclosed literal can hide the `}` that the scan looks for.
    fn braced_pattern(
        &mut self,
        path: Span,
        base: Height,
        reach: &mut Height,
    ) -> Result<Option<Diagnostic>, Diagnostic> {
        let open = self.pos;
        let mut depth = 0;
        let close = self.tokens[open..]
            .iter()
            .position(|t| {
                // The scan starts on the `{`, and the lexer rejects a closer with no opener.
                depth = nested(&t.kind, depth).unwrap_or(0);
                depth == 0
            })
            .map(|at| open + at);
        let end = close.map_or(usize::MAX, |at| self.tokens[at].span.end());
        if let Some(lexed) = self.lexer_in(self.tokens[open].span.start()..end).next() {
            return Err(lexed.clone());
        }
        if !close.is_some_and(|at| self.pattern_ends(at + 1)) {
            return Ok(None);
        }
        let (patterns, _, braces) = self.sub_patterns(base, reach, &TokenKind::RBrace)?;
        // The fix replaces the source as written, so it would delete a comment in a gap: between
        // the `{`, the sub-patterns (a label is in a gap) and the `}`.
        let after = iter::once(braces.start() + 1).chain(patterns.iter().map(|p| p.span.end()));
        let next = patterns
            .iter()
            .map(|p| p.span.start())
            .chain(iter::once(braces.end() - 1));
        let fix = after
            .zip(next)
            .all(|(after, next)| self.removal_end(after, next) == next)
            .then(|| {
                // The message goes on one line.
                let fields = Self::listed(&patterns, |span| Cow::Borrowed(self.source.text(span)));
                (Span::new(path.end(), braces.end()), fields)
            });
        Ok(Some(
            DiagnosticKind::PatternForm {
                form: ForeignPattern::Braces {
                    form: self.one_line_form(path, &patterns),
                    fix,
                },
                at: braces,
            }
            .into(),
        ))
    }

    /// Whether a pattern can end before the token at index `at`, in its place:
    ///
    /// | Place | The next token | A line break, then |
    /// | --- | --- | --- |
    /// | a sub-pattern | `,` `)` `}` `|` | `,` `}` `|` |
    /// | a match arm | `=>` `->` `if` `|` | `=>` `|` |
    /// | a `let`, `if let` or `while let` | `=` `:` `|` | none |
    ///
    /// The tokens after a line break are those that cannot start a statement. An `if` after a
    /// line break starts a statement, so it is no end. The place of an outer pattern does not
    /// count in a sub-pattern.
    fn pattern_ends(&self, at: usize) -> bool {
        let kind = &self.tokens[at].kind;
        let line_break = kind == &TokenKind::Newline;
        let next = if line_break {
            &self.tokens[self.after_line_breaks(at)].kind
        } else {
            kind
        };
        match self.pattern_place {
            PatternPlace::Sub => {
                matches!(next, TokenKind::Comma | TokenKind::RBrace | TokenKind::Pipe)
                    || (!line_break && next == &TokenKind::RParen)
            }
            PatternPlace::Arm => {
                matches!(next, TokenKind::FatArrow | TokenKind::Pipe)
                    || (!line_break && matches!(next, TokenKind::Arrow | TokenKind::If))
            }
            PatternPlace::Let => {
                !line_break && matches!(next, TokenKind::Eq | TokenKind::Colon | TokenKind::Pipe)
            }
        }
    }

    /// The index of the first token at or after `at` that is no line break. `Eof` is none, so the
    /// search ends there. A `;` that separates is no line break.
    fn after_line_breaks(&self, at: usize) -> usize {
        let skipped = self.tokens[at..].iter().take_while(|t| t.is_line_break());
        at + skipped.count()
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
    /// even when its end does not parse. A range with no end, where the pattern ends, is an error
    /// that says so.
    fn range_end(
        &mut self,
        start: Bound,
        start_span: Span,
        op: RangeOp,
    ) -> Result<Pattern, Diagnostic> {
        let dots = self.last_span();
        let at = self.pos;
        let end = self.range_bound();
        if let RangeOp::Foreign { at, lt } = op {
            let end = end.ok().map(|(_, span)| span);
            return Err(self.foreign_range(at, lt, Some(start_span), end, true));
        }
        let (end, end_span) = match end {
            Ok(end) => end,
            Err(_) if self.pattern_ends(at) => {
                return Err(self.range_bound_missing(
                    Some(start_span),
                    op,
                    None,
                    start_span.to(dots),
                ));
            }
            Err(error) => return Err(error),
        };
        Ok(Pattern {
            kind: PatternKind::Range {
                start,
                end,
                inclusive: matches!(op, RangeOp::Inclusive),
            },
            span: start_span.to(end_span),
        })
    }

    /// The bound of a range pattern at the next token: a name, unless it starts a path or a call,
    /// or a literal, and its span.
    fn range_bound(&mut self) -> Result<(Bound, Span), Diagnostic> {
        let token = self.peek();
        match token.kind {
            TokenKind::Ident(name)
                if !matches!(
                    self.tokens[self.pos + 1].kind,
                    TokenKind::LParen | TokenKind::Dot
                ) =>
            {
                self.bump();
                Ok((
                    Bound::Name(Ident {
                        name,
                        span: token.span,
                    }),
                    token.span,
                ))
            }
            _ => self
                .literal()
                .map(|(literal, span)| (Bound::Literal(literal), span)),
        }
    }

    /// The error for a range pattern with no start, at its operator `..` or `..=` at the next
    /// token. The end is read when one is there. The caller reports it.
    fn range_without_start(&mut self) -> Diagnostic {
        let dots = self.peek().span;
        let Some(op) = self.range_op() else {
            unreachable!("the next token should be a range operator")
        };
        let end = self.range_bound().ok().map(|(_, span)| span);
        if let RangeOp::Foreign { at, lt } = op {
            return self.foreign_range(at, lt, None, end, false);
        }
        let at = end.map_or(dots, |end| dots.to(end));
        self.range_bound_missing(None, op, end, at)
    }

    /// The error for a range pattern at `at` with one bound: `start` and `end` are the bounds
    /// that are there, and `op` is its operator.
    fn range_bound_missing(
        &self,
        start: Option<Span>,
        op: RangeOp,
        end: Option<Span>,
        at: Span,
    ) -> Diagnostic {
        let text = |span: Span| self.source.text(span).to_string();
        DiagnosticKind::RangeBoundMissing {
            start: start.map(text),
            inclusive: matches!(op, RangeOp::Inclusive),
            end: end.map(text),
            at,
        }
        .into()
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
    /// `<` of `..<`, and `start` and `end` its bounds, each `None` when it is missing or did not
    /// parse.
    /// Removing the `<` is a safe fix only where `fixable` and the end parsed: where the range is
    /// not allowed, or the end is broken, it leaves an error.
    fn foreign_range(
        &self,
        at: Span,
        lt: Option<Span>,
        start: Option<Span>,
        end: Option<Span>,
        fixable: bool,
    ) -> Diagnostic {
        let start_text = start.map(|start| self.line_text(start).into_owned());
        let end_text = end.map(|end| self.line_text(end).into_owned());
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

    /// A lone `|` where an operand should stand: `||` is no prefix operator, so it has no fix.
    fn prefix_pipe(&self) -> Diagnostic {
        let at = self.peek().span;
        DiagnosticKind::UnexpectedCharacter { c: '|', at }.into()
    }

    /// A lone `&` or `|` after a complete operand. Logical and and logical or are `&&` and
    /// `||`, so the report offers them, but only when an operand follows: `a & )` is no `a && )`,
    /// and `if a & {` is no `if a && {`, because that `{` opens the body.
    fn lone_infix(&self) -> Diagnostic {
        let at = self.peek().span;
        let c = if self.peek().kind == TokenKind::Amp {
            '&'
        } else {
            '|'
        };
        // The `&` or `|` is not `Eof`, so the token after it exists.
        if starts_operand(&self.tokens[self.pos + 1].kind) {
            DiagnosticKind::LogicalOperatorWritten { c, at }.into()
        } else {
            DiagnosticKind::UnexpectedCharacter { c, at }.into()
        }
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
        base: Height,
        reach: &mut Height,
        close: &TokenKind,
    ) -> Result<(Vec<Pattern>, Vec<(Span, Span)>, Span), Diagnostic> {
        let open = self.bump().span;
        let mut fields = Vec::new();
        let mut labels = Vec::new();
        while !self.at(close) {
            if let Some(label) = self.pattern_label() {
                labels.push(label);
            }
            fields.push(self.child(base, reach, |p| p.pattern_in(PatternPlace::Sub))?);
            // A line break can end a sub-pattern in a `{ }` list, before its `,` or `}`.
            while self.peek().is_line_break() {
                self.bump();
            }
            if self.eat(&TokenKind::Comma).is_none() {
                break;
            }
        }
        let close = self.expect(close)?.span;
        Ok((fields, labels, open.to(close)))
    }

    /// The end of a removal whose code ends at byte `after`, when the next token starts at byte
    /// `next`: see `source::removal_end`. Unlike `mut_not_argument`, which removes spaces and tabs
    /// only, it keeps a line break in the removal. A comment before the last removed token
    /// (`x // c`, then `: a`) is still lost.
    fn removal_end(&self, after: usize, next: usize) -> usize {
        removal_end(self.comments, after, next)
    }

    /// The E0018 for the `mut self` at the next token, in a method of `owner`.
    fn mut_self_error(&mut self, owner: Owner) -> Diagnostic {
        // `self` is not `Eof`, so the token after it exists.
        let after = &self.tokens[self.pos + 2].kind;
        let keyword = self.peek().span;
        let receiver = self.tokens[self.pos + 1].span;
        let remove = Span::new(
            keyword.start(),
            self.removal_end(keyword.end(), receiver.start()),
        );
        let safe = matches!(after, TokenKind::Comma | TokenKind::RParen);
        let annotation = if matches!(after, TokenKind::Colon) {
            self.mut_self_type()
        } else {
            None
        };
        DiagnosticKind::MutSelf {
            keyword,
            remove,
            annotation,
            safe,
            owner,
        }
        .into()
    }

    /// The `: Type` of a `mut self: Type` at `mut`, when the type parses and a `,` or a `)`
    /// follows it. The span runs to the end of the text that the type consumed, as the node span
    /// of `(i64)` leaves its `)` out. The error of a type that does not parse is dropped: the
    /// caller reports the `mut`, and `checked` resumes at the end of the item.
    fn mut_self_type(&mut self) -> Option<Span> {
        // `mut`, `self` and `:` are not `Eof`, so the tokens up to the type exist.
        let colon = self.tokens[self.pos + 2].span;
        self.pos += 3;
        self.parse_type().ok()?;
        matches!(self.peek().kind, TokenKind::Comma | TokenKind::RParen)
            .then(|| Span::new(colon.start(), self.consumed_end()))
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
        let end = self.removal_end(colon.span.end(), next.span.start());
        Some((label.span, Span::new(label.span.start(), end)))
    }

    /// `patterns` in parentheses, each as `text` gives it: `(x, y)`.
    fn listed<'t>(patterns: &[Pattern], text: impl Fn(Span) -> Cow<'t, str>) -> String {
        let written: Vec<Cow<'t, str>> = patterns.iter().map(|p| text(p.span)).collect();
        format!("({})", written.join(", "))
    }

    /// The pattern `path(patterns)` on one line, as a message quotes it.
    fn one_line_form(&self, path: Span, patterns: &[Pattern]) -> String {
        // No line break in a pattern ends a statement, so each text is one line.
        let one_line = |span| self.source.line_text(span, self.comments, &[]);
        let fields = Self::listed(patterns, one_line);
        format!("{}{fields}", one_line(path))
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
                    name: self.source.text(name).to_string(),
                    member: self.source.text(member.span).to_string(),
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
        | StmtKind::Let { init: tail, .. }
        | StmtKind::Assign { value: tail, .. }
        | StmtKind::CompoundAssign { value: tail, .. }) = &stmt.kind
        else {
            return error;
        };
        let tail = match &tail.kind {
            ExprKind::Return {
                value: Some(value), ..
            } => value.as_ref(),
            _ => tail,
        };
        let switch = match &tail.kind {
            ExprKind::Name(name) if name.as_str() == "switch" => {
                let recovered = self.recovered.len();
                let expr = self.expr().is_ok();
                self.recovered.truncate(recovered);
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
        let text = self.source.text(token.span);
        let (form, at) = match (&token.kind, &next.kind) {
            (TokenKind::Ident(_), _) if text == "case" => {
                // A comment or a line break after `case` does not end the arm: `case` and a line
                // break and then `=>` is a binding. A `;` on its line ends it.
                let at = self.after_line_breaks(self.pos + 1);
                let next = &self.tokens[at];
                // Swift's `case let .some(x)` also loses its `let` and its `.`. A `let` is not
                // `Eof`, so the token after it exists.
                let end = self.removal_end(token.span.end(), next.span.start());
                let mut remove = vec![Span::new(token.span.start(), end)];
                let mut after = next;
                let with_let = next.kind == TokenKind::Let;
                if with_let {
                    after = &self.tokens[at + 1];
                    let end = self.removal_end(next.span.end(), after.span.start());
                    remove.push(Span::new(next.span.start(), end));
                }
                let mut subject = after;
                if after.kind == TokenKind::Dot {
                    remove.push(after.span);
                    // A `.` is not `Eof`, so the token after it exists.
                    subject = &self.tokens[at + 1 + usize::from(with_let)];
                }
                // `case` is Swift's idiom only when a pattern follows. A `;` ends the arm, so it
                // keeps the fix. Else `case` is a binding, except that a `let` after it is no
                // binding, and removing it leaves no pattern.
                let ends = matches!(subject.kind, TokenKind::Newline | TokenKind::Semicolon);
                let pattern = ends || Self::starts_pattern(&subject.kind);
                if !pattern && !with_let {
                    return None;
                }
                (
                    MatchIdiomForm::Case {
                        remove: pattern.then_some(remove),
                    },
                    token.span,
                )
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
        let outer = self.scrutinee.replace(self.pos);
        let scrutinee = self.child(base, &mut reach, |p| p.headed(true, Self::expr));
        self.scrutinee = outer;
        let scrutinee = scrutinee?;
        self.header_body(&scrutinee)?;
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
        let pattern = self.child(base, &mut reach, |p| p.pattern_in(PatternPlace::Arm))?;
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
        let join = self
            .source
            .slice(gap.clone())
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
        if !stmt.is_jump()
            && let Stmt {
                kind: StmtKind::Expr(e),
                ..
            } = stmt
        {
            return Ok(e);
        }
        // The block is one level above the statement it holds.
        self.descend()?;
        Ok(Expr {
            kind: ExprKind::Block(Block {
                stmts: vec![stmt],
                span,
            }),
            span,
        })
    }

    /// A `let`, or with `pattern_forms` off, the `const` that reports like one: the forms with a
    /// pattern and an `else` are for `let` alone.
    fn let_stmt(&mut self, pattern_forms: bool) -> Result<StmtKind, Diagnostic> {
        let base = self.depth;
        let mut reach = base;
        self.bump();
        let keyword = self.eat(&TokenKind::Mut);
        let mutable = keyword.map(|t| t.span);
        let head = if pattern_forms {
            self.let_head()
        } else {
            LetHead::Name
        };
        match (head, keyword) {
            (LetHead::Pattern, Some(keyword)) => {
                return Err(self.expected_at(keyword, "a pattern"));
            }
            (LetHead::Pattern | LetHead::Qualified, None) => {
                return self.let_pattern(base, reach, mutable);
            }
            _ => {}
        }
        let name = self.binding()?;
        let (ty, annotation) = match self.eat(&TokenKind::Colon) {
            Some(colon) => {
                let ty = self.child(base, &mut reach, Self::parse_type)?;
                // Not `ty.span`: a grouped type keeps its inner span, which leaves the `)`.
                let annotation = Span::new(colon.span.start(), self.consumed_end());
                (Some(ty), Some(annotation))
            }
            None => (None, None),
        };
        self.expect(&TokenKind::Eq)?;
        if let Some(error) = self.label_not_on_loop() {
            return Err(error);
        }
        if self.at(&TokenKind::Mut) {
            // A `let mut` already binds a variable, and a block `const` has no `let`.
            return Err(self.mut_not_argument(pattern_forms && keyword.is_none()));
        }
        let init = self.child(base, &mut reach, Self::expr)?;
        self.depth = reach;
        // A typed `let` takes no `else`, which can start the next line.
        if let Some(at) = annotation
            && pattern_forms
            && let Some(else_at) = self.else_after(&init)
        {
            return Err(DiagnosticKind::LetTakesNoType {
                at,
                with_else: true,
                join: self.type_fix_join(else_at),
            }
            .into());
        }
        let else_follows = pattern_forms && ty.is_none() && self.at(&TokenKind::Else);
        // Reported last, and only where the statement ends, so that the fix leaves a `let` that
        // parses: past a stray token, the block reports that token's own error.
        if let Some(keyword) = mutable
            && name.name.as_str() == "_"
            && (else_follows || self.at(&TokenKind::Newline) || self.at(&TokenKind::RBrace))
        {
            return Err(DiagnosticKind::MutOnUnderscore {
                at: name.span,
                remove: Span::new(
                    keyword.start(),
                    self.removal_end(keyword.end(), name.span.start()),
                ),
            }
            .into());
        }
        if else_follows {
            let pattern = if name.name.as_str() == "_" {
                PatternKind::Wild
            } else {
                PatternKind::Name(name.clone())
            };
            let pattern = Pattern {
                kind: pattern,
                span: name.span,
            };
            return self.let_else_block(base, reach, mutable, pattern, init);
        }
        Ok(StmtKind::Let {
            mutable,
            name,
            ty,
            init,
        })
    }

    /// What the tokens after `let` and `mut` read as.
    fn let_head(&self) -> LetHead {
        use TokenKind as T;
        let kind = |n: usize| self.tokens.get(self.pos + n).map(|t| &t.kind);
        match self.peek().kind {
            T::NoneKw
            | T::Minus
            | T::Int(_)
            | T::Char(_)
            | T::Str(_)
            | T::StrHead(_)
            | T::True
            | T::False => LetHead::Pattern,
            T::Ident(_) => match kind(1) {
                // A `{` after the name is Rust's `Rect { w, h }`, which `single` reports.
                Some(T::LParen | T::LBrace | T::Pipe | T::DotDot | T::DotDotEq) => LetHead::Pattern,
                Some(T::Dot) if matches!(kind(2), Some(T::Ident(_) | T::NoneKw)) => match kind(3) {
                    Some(T::LParen | T::LBrace | T::Pipe) => LetHead::Pattern,
                    Some(T::Eq | T::Colon) => LetHead::Qualified,
                    _ => LetHead::Name,
                },
                _ => LetHead::Name,
            },
            _ => LetHead::Name,
        }
    }

    /// `pattern = value`, then `else` and a block: the rest of a `let` whose pattern is not a
    /// plain name. Without the `else`, the statement keeps `else_block: None`: the checker reports
    /// it by the value's type, which the parser does not know. A qualified variant with no payload
    /// keeps the error of a `let` that reads a name.
    fn let_pattern(
        &mut self,
        base: Height,
        mut reach: Height,
        mutable: Option<Span>,
    ) -> Result<StmtKind, Diagnostic> {
        let first = self.pos;
        let pattern = self.child(base, &mut reach, |p| p.pattern_in(PatternPlace::Let))?;
        let qualified = matches!(
            &pattern.kind,
            PatternKind::Variant {
                qualifier: Some(_),
                parens: None,
                ..
            }
        );
        if let Some(colon) = self.eat(&TokenKind::Colon) {
            // With no `else` on this line or the next, it is no `let ... else`, so the `.` is the
            // first mistake, before the type.
            if qualified && self.else_in_statement(first, true).is_none() {
                return Err(self.dot_error(first));
            }
            self.child(base, &mut reach, Self::parse_type)?;
            let at = Span::new(colon.span.start(), self.consumed_end());
            // With an `else` on the line, the fix leaves it as it is. With one on a later line
            // and a value after the type, the fix joins it to the value.
            let same_line = self.else_in_statement(first, false).is_some();
            let join = if same_line || !self.at(&TokenKind::Eq) {
                None
            } else {
                self.else_in_statement(first, true)
                    .and_then(|else_at| self.type_fix_join(else_at))
            };
            return Err(DiagnosticKind::LetTakesNoType {
                at,
                with_else: same_line || join.is_some(),
                join,
            }
            .into());
        }
        self.expect(&TokenKind::Eq)?;
        if let Some(error) = self.label_not_on_loop() {
            return Err(error);
        }
        if self.at(&TokenKind::Mut) {
            return Err(self.mut_not_argument(true));
        }
        let value = match self.child(base, &mut reach, Self::expr) {
            Ok(value) => value,
            // With no `else` in the statement, it is no `let ... else`, so the `.` is the first
            // mistake. With one, the pattern is right and the value's error is the mistake.
            Err(_) if qualified && self.else_in_statement(first, false).is_none() => {
                return Err(self.dot_error(first));
            }
            Err(error) => return Err(error),
        };
        self.depth = reach;
        if self.at(&TokenKind::Else) {
            return self.let_else_block(base, reach, mutable, pattern, value);
        }
        // The pattern and its value, then an `else` on the next line: the `let ... else` that
        // E0006 reports, which comes before the error of a pattern that can fail.
        if let Some(at) = self.else_after(&value) {
            return Err(self.let_else_on_new_line(at));
        }
        if qualified {
            return Err(self.dot_error(first));
        }
        Ok(StmtKind::LetElse {
            mutable,
            pattern,
            value,
            else_block: None,
        })
    }

    /// The token index of an `else` after `value`, the value of a `let`, on its line or on a later
    /// one. A value that ends in an `if` without `else` keeps it: the `else` is that `if`'s.
    fn else_after(&self, value: &Expr) -> Option<usize> {
        self.tokens[self.pos..]
            .iter()
            .position(|t| t.kind != TokenKind::Newline)
            .map(|n| self.pos + n)
            .filter(|&at| self.tokens[at].kind == TokenKind::Else && !ends_in_open_if(value))
    }

    /// The token index of the last `else` of the statement that starts at token `first` that is
    /// not the `else` of an `if` in it: one at the statement's own level, before its line break,
    /// or with `next_line` after it. `None` when there is none. A line break inside `{ }` does
    /// not end the statement. It runs on an error path only.
    fn else_in_statement(&self, first: usize, next_line: bool) -> Option<usize> {
        let (mut depth, mut elses, mut ifs) = (0_usize, 0_usize, 0_usize);
        let mut last = None;
        for (at, token) in self.tokens[first..].iter().enumerate() {
            // A closing bracket at the statement's own level ends the statement.
            let Some(next) = nested(&token.kind, depth) else {
                break;
            };
            depth = next;
            match token.kind {
                TokenKind::Newline if depth == 0 => {
                    let after = self.tokens[first + at..]
                        .iter()
                        .position(|t| t.kind != TokenKind::Newline)
                        .map(|n| first + at + n)
                        .filter(|&n| next_line && self.tokens[n].kind == TokenKind::Else);
                    if after.is_some() {
                        elses += 1;
                        last = after;
                    }
                    break;
                }
                TokenKind::Else if depth == 0 => {
                    elses += 1;
                    last = Some(first + at);
                }
                TokenKind::If if depth == 0 => ifs += 1,
                _ => {}
            }
        }
        last.filter(|_| elses > ifs)
    }

    /// "expected `=`, found `.`", as a `let` that reads a name reports `Enum.Variant`. The
    /// pattern starts at token `first`.
    fn dot_error(&self, first: usize) -> Diagnostic {
        self.expected_at(&self.tokens[first + 1], &TokenKind::Eq.describe())
    }

    /// The `else` and the block of a `let ... else`, after its `value`. A value that ends in `}`
    /// is an error: the `else` would read as the value's own.
    fn let_else_block(
        &mut self,
        base: Height,
        mut reach: Height,
        mutable: Option<Span>,
        pattern: Pattern,
        value: Expr,
    ) -> Result<StmtKind, Diagnostic> {
        if self.tokens[self.pos - 1].kind == TokenKind::RBrace {
            return Err(DiagnosticKind::LetElseValueEndsInBrace { at: value.span }.into());
        }
        self.bump();
        let else_block = self.child(base, &mut reach, Self::block)?;
        self.depth = reach;
        Ok(StmtKind::LetElse {
            mutable,
            pattern,
            value,
            else_block: Some(else_block),
        })
    }

    /// The error of a `const` statement. It is reported only once the statement parses as a `let`
    /// and ends the line, so that its fix leaves a `let` that parses; before that, the first
    /// error in it is reported.
    fn local_const(&mut self) -> Diagnostic {
        let at = self.peek().span;
        if let Err(error) = self.let_stmt(false) {
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
            return Err(DiagnosticKind::AssignToSelf {
                at: target.span,
                struct_self: self.struct_self,
            }
            .into());
        }
        if !is_place(&target) && !is_range_target(&target) {
            return Err(DiagnosticKind::AssignToExpression {
                at: target.span,
                range: None,
            }
            .into());
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
        self.range_expr(false)
    }

    /// Runs `parse` with `header` set to `header`, and restores it after, also on an error.
    fn headed<T>(&mut self, header: bool, parse: impl FnOnce(&mut Self) -> T) -> T {
        let outer = mem::replace(&mut self.header, header);
        let parsed = parse(self);
        self.header = outer;
        parsed
    }

    /// An expression, or a range `a..b`, `a..=b`, `a..`, `..b`, `..=b` or `..`, which binds looser
    /// than every operator. The checker reports a range where it is not allowed. `source` is the
    /// source of a `for`: only there does another language's range, such as `0..<n`, have a fix.
    fn range_expr(&mut self, source: bool) -> Result<Expr, Diagnostic> {
        self.descend()?;
        let base = self.depth;
        let start = if self.at_range_op() {
            None
        } else {
            let start = self.binary(1)?;
            if !self.at_range_op() {
                return Ok(start);
            }
            Some(start)
        };
        let dots = self.peek().span;
        let pos = self.pos;
        let Some(op) = self.range_op() else {
            unreachable!("the next token should be a range operator")
        };
        if let RangeOp::Foreign { at, lt } = op {
            let Some(start) = start else {
                // Nothing to write the fix from: the error is the operand that is missing.
                self.pos = pos;
                return self.binary(1);
            };
            let end = self.binary(1).ok().map(|end| end.span);
            return Err(self.foreign_range(at, lt, Some(start.span), end, source));
        }
        // The range is the parent of its bounds: one level more, as an operator is.
        let wrapped = self.depth.link();
        self.reach(wrapped)?;
        self.depth = base.link();
        let end = if range_end_starts(&self.peek().kind, self.header) {
            Some(self.binary(1)?)
        } else {
            None
        };
        self.depth = self.depth.max(wrapped);
        let span = start
            .as_ref()
            .map_or(dots, |start| start.span)
            .to(self.last_span());
        let range = RangeExpr {
            start: start.map(Box::new),
            end: end.map(Box::new),
            inclusive: matches!(op, RangeOp::Inclusive),
            span,
        };
        Ok(Expr {
            span,
            kind: ExprKind::Range(range),
        })
    }

    /// Whether the next token is `..` or `..=`.
    fn at_range_op(&self) -> bool {
        matches!(self.peek().kind, TokenKind::DotDot | TokenKind::DotDotEq)
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
                let wrapped = self.depth.link();
                self.reach(wrapped)?;
                let op_span = self.bump().span;
                self.depth = base.link();
                // `??` is right-associative: its right side starts at its own level.
                let rhs = if matches!(op, Infix::Coalesce) {
                    self.binary(level)?
                } else {
                    self.binary(level + 1)?
                };
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
            } else if matches!(kind, TokenKind::Amp | TokenKind::Pipe) {
                return Err(self.lone_infix());
            } else {
                return Ok(lhs);
            }
        }
    }

    /// The index of `base[`, whose `[` is `open`, up to the `]`, and the span of the `]`. A range
    /// written with `:`, as Python and Go write it, is reported and read as the range it names;
    /// `[:]` gives `None`, which is the whole `base`. A step is the error of the `]` that is
    /// missing.
    fn index(&mut self, base: &Expr, open: Span) -> Result<(Option<Expr>, Span), Diagnostic> {
        let start = if self.at(&TokenKind::Colon) {
            None
        } else {
            Some(self.expr()?)
        };
        // A bound that is a range has no colon form: `xs[0..1:2]` is a step to the `]` check.
        let ranged = matches!(start.as_ref().map(|e| &e.kind), Some(ExprKind::Range(_)));
        let colon = if ranged {
            None
        } else {
            self.eat(&TokenKind::Colon).map(|colon| colon.span)
        };
        let Some(colon) = colon else {
            let close = self.expect(&TokenKind::RBracket)?.span;
            return Ok((start, close));
        };
        // Not `expr`: `xs[0:1..2]` reads `1` and meets the `..` where the `]` should be.
        let end = if range_end_starts(&self.peek().kind, false) {
            Some(self.binary(1)?)
        } else {
            None
        };
        let close = self.expect(&TokenKind::RBracket)?.span;
        let bound = |bound: &Option<Expr>| bound.as_ref().map(|e| self.colon_bound(e));
        let form = ForeignRange::Colon {
            base: self.line_text(base.span).into_owned(),
            base_place: is_place(base) && is_named(base),
            start: bound(&start),
            end: bound(&end),
            whole: (start.is_none() && end.is_none()).then(|| open.to(close)),
        };
        // A negative bound has no fix that replaces the colon: the report points at the bound.
        let at = [&start, &end]
            .into_iter()
            .flatten()
            .find(|bound| starts_negative(bound))
            .map_or(colon, |bound| bound.span);
        self.recovered
            .push(DiagnosticKind::ForeignRange { form, at }.into());
        if start.is_none() && end.is_none() {
            return Ok((None, close));
        }
        let span = start.as_ref().map_or(colon, |start| start.span);
        let span = span.to(end.as_ref().map_or(colon, |end| end.span));
        let range = RangeExpr {
            start: start.map(Box::new),
            end: end.map(Box::new),
            inclusive: false,
            span,
        };
        let index = Expr {
            span,
            kind: ExprKind::Range(range),
        };
        Ok((Some(index), close))
    }

    /// The bound `e` of `base[start:end]`: Python counts a negative one from the end.
    fn colon_bound(&self, e: &Expr) -> ColonBound {
        if starts_negative(e) {
            ColonBound::Negative
        } else {
            ColonBound::Plain(self.line_text(e.span).into_owned())
        }
    }

    /// The error for the `**` that is next, after `lhs`: the right side parses as `*`'s would,
    /// and a `=` straight after the `**` is part of the error, so `x **= 2` is reported once.
    /// The unsafe fix is `lhs.pow(rhs)`: offered when both sides are plain operands, and not
    /// for `**=` or a chain of `**`, which Python reads from the right.
    fn power_operator(&mut self, lhs: &Expr, base: Height) -> Diagnostic {
        let left_ok = power_operand(lhs, self.operand_tokens(lhs), false);
        let at = self.bump().span;
        let assign = self.eat(&TokenKind::Eq).is_some();
        self.depth = base.link();
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

    /// Level 4: `first` and every `<`/`<=`/`>`/`>=` operand after it, as one node, which is one
    /// link of the chain of operators that ends with it.
    fn comparison_chain(&mut self, first: Expr, base: Height) -> Result<Expr, Diagnostic> {
        let start = first.span;
        // The chain is the parent of `first`: one level more.
        let mut reach = self.depth.link();
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
            self.depth = base.link();
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
            TokenKind::Mut => return Err(self.mut_not_argument(false)),
            _ => return self.postfix(),
        };
        // Each prefix operator is a link of a chain.
        self.reach(self.depth.link())?;
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
            let wrapped = self.depth.link();
            if self.at(&TokenKind::LBracket) {
                self.reach(wrapped)?;
                let open = self.bump().span;
                self.depth = base.link();
                let (index, close) = self.headed(false, |p| p.index(&expr, open))?;
                self.depth = self.depth.max(wrapped);
                // `xs[:]` is `xs`.
                if let Some(index) = index {
                    expr = Expr {
                        span: expr.span.to(close),
                        kind: ExprKind::Index {
                            base: Box::new(expr),
                            index: Box::new(index),
                        },
                    };
                }
                continue;
            }
            // `[` was handled above, so only `.` and `?.` reach here.
            if !starts_postfix(&self.peek().kind) {
                return Ok(expr);
            }
            self.reach(wrapped)?;
            let access = self.bump();
            let safe = access.kind == TokenKind::QuestionDot;
            // `Option.None` names the variant of the prelude's `Option`; the checker rejects the
            // `None` member of anything else.
            let name = if !safe && self.at(&TokenKind::NoneKw) {
                Ident {
                    name: Symbol::NONE,
                    span: self.bump().span,
                }
            } else {
                self.ident("a field or method name")?
            };
            // The type arguments and the value arguments are siblings of the receiver: each
            // starts at the receiver's own level and only the deepest counts.
            let written = if self.at(&TokenKind::Lt) && self.type_args_follow() {
                self.depth = base.link();
                let (types, angles) = self.type_args()?;
                if !self.at(&TokenKind::LParen) {
                    // A field takes no type arguments, so `a.b<i64>.c` has nowhere to put them.
                    return Err(self.expected("`(`"));
                }
                Some((types, angles, self.depth.max(wrapped)))
            } else {
                None
            };
            // Without type arguments a field access holds no list to drop.
            expr = if written.is_some() || self.at(&TokenKind::LParen) {
                let (types, angles, types_reach) =
                    written.unwrap_or_else(|| (Vec::new(), name.span.end_point(), wrapped));
                self.depth = base.link();
                let open = self.peek().span;
                let (args, close) = self.args()?;
                self.depth = self.depth.max(types_reach);
                Expr {
                    span: expr.span.to(close),
                    kind: ExprKind::MethodCall {
                        receiver: Box::new(expr),
                        method: name,
                        types,
                        angles,
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
                // The type arguments and the value arguments are siblings: each starts at the
                // callee's level and only the deepest counts.
                let base = self.depth;
                let written = if self.at(&TokenKind::Lt) && self.type_args_follow() {
                    Some(self.type_args()?)
                } else {
                    None
                };
                let types_reach = self.depth;
                let callee = Ident {
                    name,
                    span: token.span,
                };
                if self.at(&TokenKind::LParen) {
                    let (types, angles) =
                        written.unwrap_or_else(|| (Vec::new(), token.span.end_point()));
                    self.depth = base;
                    let (args, close) = self.args()?;
                    self.depth = self.depth.max(types_reach);
                    return Ok(Expr {
                        span: token.span.to(close),
                        kind: ExprKind::Call {
                            callee,
                            types,
                            angles,
                            args,
                        },
                    });
                }
                // A plain name holds no list to drop.
                if let Some((types, angles)) = written {
                    return Ok(Expr {
                        span: token.span.to(self.last_span()),
                        kind: ExprKind::Applied {
                            name: callee,
                            types,
                            angles,
                        },
                    });
                }
                ExprKind::Name(name)
            }
            TokenKind::Todo => return self.todo(),
            TokenKind::Return | TokenKind::Break | TokenKind::Continue => {
                let base = self.depth;
                let mut reach = base;
                let jump = self.jump(base, &mut reach)?;
                self.depth = reach;
                return Ok(jump);
            }
            TokenKind::LParen => {
                self.bump();
                let inner = self.headed(false, Self::expr)?;
                let close = self.expect(&TokenKind::RParen)?;
                // The parentheses are part of the span, so diagnostics underline them.
                return Ok(Expr {
                    span: token.span.to(close.span),
                    ..inner
                });
            }
            TokenKind::LBracket => {
                self.bump();
                return self.headed(false, |p| p.array(token.span));
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
            TokenKind::Pipe => return Err(self.prefix_pipe()),
            _ => {
                debug_assert!(
                    !starts_operand(&token.kind),
                    "`starts_operand` should list only tokens that `primary` takes"
                );
                return Err(self.expected("an expression"));
            }
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
            && self
                .source
                .text(next.span)
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
        let rest = self.source.rest(delete.end());
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
                bad_spec: false,
            })
        };
        let base = self.depth;
        let mut reach = base;
        let mut parts: Vec<Part> = text(head, start).into_iter().collect();
        let end = loop {
            let value = self.child(base, &mut reach, |p| p.headed(false, Self::expr))?;
            let mut spec = None;
            let mut bad_spec = false;
            if let TokenKind::FormatSpec(written) = &self.peek().kind {
                let at = self.bump().span;
                spec = format_spec(written, at)
                    .map_err(|error| {
                        self.recovered.push(error);
                        bad_spec = true;
                    })
                    .ok();
            }
            parts.push(Part {
                value,
                spec,
                bad_spec,
            });
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
                args.push(self.child(base, &mut reach, |p| p.headed(false, Self::arg))?);
                if self.eat(&TokenKind::Comma).is_none() {
                    break;
                }
            }
        }
        let close = self.expect(&TokenKind::RParen)?;
        self.depth = reach;
        Ok((args, close.span))
    }

    /// A label is an `Ident` followed by `:`, so it takes two tokens of lookahead. A `mut` goes
    /// before the label or before the value.
    fn arg(&mut self) -> Result<Arg, Diagnostic> {
        let mut mut_keyword = self.eat(&TokenKind::Mut).map(|t| t.span);
        let labeled = matches!(self.peek().kind, TokenKind::Ident(_))
            && self
                .tokens
                .get(self.pos + 1)
                .is_some_and(|t| t.kind == TokenKind::Colon);
        let (label, colon) = if labeled {
            let label = self.ident("a label")?;
            let colon = self.bump().span;
            if let Some(error) = self.label_not_on_loop() {
                return Err(error);
            }
            mut_keyword = mut_keyword.or_else(|| self.eat(&TokenKind::Mut).map(|t| t.span));
            (Some(label), Some(colon))
        } else {
            (None, None)
        };
        Ok(Arg {
            label,
            colon,
            mut_keyword,
            value: self.expr()?,
        })
    }

    /// E0168 for the `mut` at the next token, which no argument owns. `binding` is whether it
    /// stands right after the `=` of a `let` that has no `mut`. The fix removes the keyword and
    /// the spaces and tabs after it, and no comment or line break.
    fn mut_not_argument(&self, binding: bool) -> Diagnostic {
        let at = self.peek().span;
        // `mut` is never `Eof`, so the token after it exists.
        let next = &self.tokens[self.pos + 1];
        let gap = self.source.slice(at.end()..next.span.start());
        let fix_end = at.end() + gap.len() - gap.trim_start_matches([' ', '\t']).len();
        DiagnosticKind::MutNotArgument {
            at,
            binding,
            fix_end,
        }
        .into()
    }

    fn if_expr(&mut self) -> Result<Expr, Diagnostic> {
        let start = self.expect(&TokenKind::If)?.span;
        let base = self.depth;
        let mut reach = base;
        let head = match self.child(base, &mut reach, Self::let_binding)? {
            Some((pattern, value)) => Head::Let(pattern, value),
            None => Head::Cond(self.child(base, &mut reach, |p| p.headed(true, Self::expr))?),
        };
        self.header_body(match &head {
            Head::Cond(e) | Head::Let(_, e) => e,
        })?;
        let then_block = self.child(base, &mut reach, Self::block)?;
        let mut end = then_block.span;
        let else_block = if let Some(else_token) = self.eat(&TokenKind::Else) {
            let block = if self.at(&TokenKind::If) {
                // `else if` and `else if let` become a block holding the inner `if`.
                let else_if = else_token.span.to(self.peek().span);
                let inner = self.child(base, &mut reach, |p| {
                    p.reach(p.depth.else_if(else_if))?;
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
        self.open_loop(label.as_ref(), false);
        let body = self.child(base, &mut reach, Self::block);
        // The loop part and the loop label close with the body, also when it fails to parse.
        let breaks = self.loops.pop().is_some_and(|part| part.left);
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
                breaks,
            },
        })
    }
}

/// A name, a field of any expression, or an index of a place by an `i64`: a range index gives a
/// new value, so it is no place.
pub(crate) fn is_place(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Name(_) | ExprKind::Field { .. } => true,
        ExprKind::Index { base, index } => {
            !matches!(index.kind, ExprKind::Range(_)) && is_place(base)
        }
        _ => false,
    }
}

/// Whether the leftmost operand of `e` is a unary minus or a negative literal: `-1`, `-a * b`,
/// `-1 + n` and `--1` are, `n - 1` is not.
fn starts_negative(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Unary {
            op: UnaryOp::Neg, ..
        } => true,
        ExprKind::Int { value, .. } => *value < 0,
        ExprKind::Binary { lhs, .. }
        | ExprKind::Coalesce { lhs, .. }
        | ExprKind::Is { lhs, .. } => starts_negative(lhs),
        ExprKind::Compare { operands, .. } => operands.first().is_some_and(starts_negative),
        _ => false,
    }
}

/// Whether the place `e` starts at a name or `self`, so that to write it twice is to read the same
/// thing twice: `f().x` is a place, and `f` would run twice.
fn is_named(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Name(_) | ExprKind::SelfValue => true,
        ExprKind::Field { base, .. } | ExprKind::Index { base, .. } => is_named(base),
        _ => false,
    }
}

/// A range index, or an index of one, as `xs[1..3]` and `xs[1..][0]`. It is no place, but the
/// checker, which knows the type of `xs`, reports the assignment to it.
pub(crate) fn is_range_target(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Index { base, index } => {
            matches!(index.kind, ExprKind::Range(_)) || is_range_target(base)
        }
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
        parse(lex(src).expect("source lexes"), false)
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
    fn format_spec_should_print_in_the_order_the_grammar_gives() {
        assert_eq!(
            expr(r#"print("a{x:>8.2}b{y}{z:.1}{w:4}")"#),
            r#"(call print (interp "a" (fmt x ">8.2") "b" y (fmt z ".1") (fmt w "4")))"#
        );
        assert_eq!(
            expr(r#"print("{x:#}")"#),
            r##"(call print (interp (fmt x "#")))"##
        );
    }

    #[test]
    fn format_spec_should_parse_each_part_and_point_each_error_at_the_spec() {
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
                (spec.align, spec.width, spec.precision, spec.pretty),
                (align, width, precision, false),
                "{text}"
            );
        }
        let pretty = format_spec("#", at).expect("#");
        assert_eq!(
            (pretty.align, pretty.width, pretty.precision, pretty.pretty),
            (None, None, None, true)
        );
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
            ("##", unsupported),
            ("#x", unsupported),
            ("#>8", unsupported),
            (">8#", unsupported),
            (">#8", unsupported),
            ("#.2", unsupported),
            ("#8", unsupported),
            ("#.", malformed),
            ("#2000", unsupported),
            ("#.31", unsupported),
            ("#99999999999999999999", unsupported),
            ("#?", unsupported),
            ("?", unsupported),
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

    /// `#` with an alignment, a width or a precision, in any order, is its own form of E0109, which
    /// has a help; two `#` or a `#` with another character are the form that every unsupported
    /// spec gets, which has none. The fixture `E0109-pretty` pins the text.
    #[test]
    fn format_spec_should_give_the_pretty_form_only_for_a_combined_hash() {
        let at = Span::new(0, 0);
        for (text, combined) in [
            ("#>8", true),
            (">8#", true),
            (">#8", true),
            ("#.2", true),
            ("#8", true),
            ("#2000", true),
            ("#.31", true),
            ("#65536.5", true),
            ("##", false),
            ("#x", false),
            ("+", false),
        ] {
            let error = format_spec(text, at).expect_err(text);
            assert_eq!(error.code(), Code::UnsupportedFormatSpec, "{text}");
            assert_eq!(error.help().is_some(), combined, "{text}");
        }
    }

    /// A `?` after a spec that is right without it, which a Rust user writes, gets a fix that
    /// deletes the `?`. Other text with a `?` is the general form, which has none. The fixtures
    /// `E0109-debug`, `E0109-debug-hash` and `E0109-debug-width` pin the text and the edit.
    #[test]
    fn format_spec_should_fix_a_question_mark_after_a_right_spec() {
        for (text, fixed) in [
            ("?", true),
            ("#?", true),
            (">8?", true),
            (".2?", true),
            ("?#", false),
            ("??", false),
            ("#?x", false),
            ("#>?", false),
            (">?", false),
            ("x?", false),
            ("2000?", false),
        ] {
            // The span starts at the `:`, and the fix needs the last byte of the spec.
            let at = Span::new(10, 11 + text.len());
            let error = format_spec(text, at).expect_err(text);
            assert_eq!(error.code(), Code::UnsupportedFormatSpec, "{text}");
            assert_eq!(error.fixes().len(), usize::from(fixed), "{text}");
        }
    }

    /// A `#` with a limit error in the rest gets a help and no fix. The fixtures `E0109-limit`
    /// and `E0109-limit-precision` pin the text.
    #[test]
    fn format_spec_should_help_a_hash_with_a_limit_error() {
        let at = Span::new(0, 0);
        for text in ["#2000", "#<2000", "#.40", "#2000.40", "#5"] {
            let error = format_spec(text, at).expect_err(text);
            assert_eq!(error.code(), Code::UnsupportedFormatSpec, "{text}");
            assert!(error.help().is_some(), "{text}");
            assert!(error.fixes().is_empty(), "{text}");
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

    /// GIVEN `return`, `break` and `continue` in the place of an operand
    /// WHEN the parser reads them
    /// THEN each is an expression, with its loop label and value, that `Display` prints as it
    /// printed the statement
    #[test]
    fn jump_should_parse_as_an_operand() {
        for (src, printed) in [
            ("o ?? return 1", "(?? o (return 1))"),
            ("o ?? break", "(?? o (break))"),
            ("o ?? break outer", "(?? o (break outer))"),
            ("o ?? continue outer", "(?? o (continue :outer))"),
            ("f(return)", "(call f (return))"),
            ("(return)", "(return)"),
            (
                "outer: loop { o ?? break outer }",
                "(loop :outer (block (?? o (break :outer))))",
            ),
            (
                "outer: loop { o ?? break outer 2 }",
                "(loop :outer (block (?? o (break :outer 2))))",
            ),
            (
                "outer: loop { o ?? continue outer }",
                "(loop :outer (block (?? o (continue :outer))))",
            ),
        ] {
            assert_eq!(expr(src), printed, "{src}");
        }
        let function = first_fn("fn main() { outer: loop { o ?? break outer 2 } }");
        let StmtKind::Expr(Expr {
            kind: ExprKind::Loop { body, .. },
            ..
        }) = &function.body.stmts[0].kind
        else {
            panic!("a labeled loop should parse as a loop");
        };
        let StmtKind::Expr(Expr {
            kind: ExprKind::Coalesce { rhs, .. },
            ..
        }) = &body.stmts[0].kind
        else {
            panic!("`o ?? break outer 2` should parse as a coalesce");
        };
        assert!(
            matches!(
                &rhs.kind,
                ExprKind::Break { label: Some(label), value: Some(value), .. }
                    if label.name.as_str() == "outer" && matches!(value.kind, ExprKind::Int { value: 2, .. })
            ),
            "{rhs:?}"
        );
        assert!(rhs.is_jump());
    }

    /// The `breaks` of the loops in `block`, outer loop first.
    fn loop_breaks(block: &Block, found: &mut Vec<bool>) {
        for stmt in &block.stmts {
            match &stmt.kind {
                StmtKind::Expr(Expr {
                    kind: ExprKind::Loop { body, breaks, .. },
                    ..
                }) => {
                    found.push(*breaks);
                    loop_breaks(body, found);
                }
                StmtKind::While { body, .. } => loop_breaks(body, found),
                _ => {}
            }
        }
    }

    /// GIVEN a `loop` with a `break`, a `return` or a `continue` in its body, or in a loop in it
    /// WHEN the parser reads it
    /// THEN a `loop` records `breaks` only when a `break` leaves that loop
    #[test]
    fn a_loop_should_record_whether_a_break_leaves_it() {
        for (src, expected) in [
            ("loop { }", vec![false]),
            ("loop { break }", vec![true]),
            ("loop { return }", vec![false]),
            ("loop { loop { break } }", vec![false, true]),
            ("outer: loop { loop { break outer } }", vec![true, false]),
            ("loop { while c { break } }", vec![false]),
            ("loop { continue }", vec![false]),
        ] {
            let function = first_fn(&format!("fn main() {{ {src} }}"));
            let mut found = Vec::new();
            loop_breaks(&function.body, &mut found);
            assert_eq!(found, expected, "{src}");
        }
    }

    /// GIVEN `a ?? return b ?? c`
    /// WHEN the parser reads it
    /// THEN `return` takes `b ?? c`, everything to its right
    #[test]
    fn return_should_take_everything_to_its_right() {
        assert_eq!(expr("a ?? return b ?? c"), "(?? a (return (?? b c)))");
    }

    /// GIVEN a `return` before each token that ends a jump
    /// WHEN the parser reads it
    /// THEN the `return` takes no value
    #[test]
    fn jump_with_no_value_should_end_at_jump_ends() {
        for (src, printed) in [
            ("fn main() {\n    return\n}", "(block (return))"),
            ("fn main() { return; 1 }", "(block (return) 1)"),
            ("fn main() { return }", "(block (return))"),
            ("fn main() { f(return, 1) }", "(block (call f (return) 1))"),
            ("fn main() { [return] }", "(block (array (return)))"),
            ("fn main() { f(return) }", "(block (call f (return)))"),
            (
                "fn main() { match x { _ if return => 1 } }",
                "(block (match x (arm _ (if (return)) 1)))",
            ),
        ] {
            assert_eq!(body(src), printed, "{src}");
        }
        // An `else` and the end of the file cannot sit inside a block, so the parser starts at
        // the `return`.
        for src in ["return else", "return"] {
            parser_at(src, 0, |p| {
                let stmt = p.stmt().expect("the statement should parse");
                assert!(
                    matches!(
                        stmt.kind,
                        StmtKind::Expr(Expr {
                            kind: ExprKind::Return { value: None, .. },
                            ..
                        })
                    ),
                    "{src}"
                );
            });
        }
    }

    /// GIVEN a `return`, `break` or `continue` before each token of an interpolation that can
    /// follow an expression
    /// WHEN the parser reads it
    /// THEN the jump takes no value
    #[test]
    fn jump_with_no_value_should_end_at_an_interpolation_part() {
        for (src, printed) in [
            (
                r#"print("{o ?? return}")"#,
                r"(call print (interp (?? o (return))))",
            ),
            (
                r#"print("{o ?? break}")"#,
                r"(call print (interp (?? o (break))))",
            ),
            (
                r#"print("{o ?? continue}")"#,
                r"(call print (interp (?? o (continue))))",
            ),
            (
                r#"print("{o ?? return}-{o ?? break}")"#,
                r#"(call print (interp (?? o (return)) "-" (?? o (break))))"#,
            ),
            (
                r#"print("{o ?? return:>4}")"#,
                r#"(call print (interp (fmt (?? o (return)) ">4")))"#,
            ),
            (
                r#"print("{o ?? break 1}")"#,
                r"(call print (interp (?? o (break 1))))",
            ),
        ] {
            assert_eq!(expr(src), printed, "{src}");
        }
    }

    /// GIVEN a `return` or a `break` statement whose value is the deepest parenthesized value
    /// that parses (254 pairs after `return`, and 252 after `break` in a `loop`)
    /// WHEN the parser reads it, and again with one pair more
    /// THEN the first parses, and the second is E0071 at the `1` inside the parentheses
    /// AND a `return` operand that deep is E0071, not a crash
    #[test]
    fn jump_statement_should_keep_its_depth_budget() {
        let source = |pairs: usize, head: &str, tail: &str| {
            format!(
                "fn main() {{ {head}{}1{}{tail} }}",
                "(".repeat(pairs),
                ")".repeat(pairs)
            )
        };
        // The code and the column of the first error.
        let first = |src: &str| {
            parsed(src)
                .diagnostics
                .first()
                .map(|d| (d.code(), line_col(src, d.span().start()).1))
        };
        // The parser recurses as deep as the limit allows, which the compiler's own stack holds.
        let found = std::thread::Builder::new()
            .stack_size(crate::STACK_SIZE)
            .spawn(move || {
                let statements = |extra: usize| {
                    [
                        first(&source(254 + extra, "return ", "")),
                        first(&source(252 + extra, "loop { break ", " }")),
                    ]
                };
                let operand = first(&source(254, "f(return ", ")")).map(|(code, _)| code);
                (statements(0), statements(1), operand)
            })
            .expect("the thread should start")
            .join()
            .expect("the parse should not overflow the stack");
        let too_deep = Code::NestingTooDeep;
        assert_eq!(
            found,
            (
                [None, None],
                [Some((too_deep, 275)), Some((too_deep, 279))],
                Some(too_deep)
            )
        );
    }

    /// GIVEN a function with the jump statements `return`, `return x`, `break`, `break l 1` and
    /// `continue l`
    /// WHEN the tree is printed
    /// THEN the text is the text of the tree at the branch base, where they were statements
    #[test]
    fn jump_statement_should_print_as_today() {
        let src = "fn f(x: i64) -> i64 {\n    l: loop {\n        return\n        return x\n        break\n        break l 1\n        continue l\n        break 1\n        continue\n    }\n}";
        assert_eq!(
            program(src),
            "(fn f ((x i64)) i64 (block (loop :l (block (return) (return x) (break) (break :l 1) (continue :l) (break 1) (continue)))))"
        );
    }

    /// GIVEN `return x = 5`
    /// WHEN the parser reads it
    /// THEN the error is E0011 at the `=`, as it was when `return` was a statement only
    #[test]
    fn jump_before_eq_should_keep_its_error() {
        assert_eq!(expr_err("return x = 5"), (Code::Expected, 1, 22));
    }

    /// GIVEN a header of each kind (`if`, `if let`, `else if`, `while`, `while let`, `for` over
    /// an array and over a range, and `match`) that ends in `return` or `break` with no value,
    /// followed by the `{` of its body
    /// WHEN the parser reads it
    /// THEN the error is E0011 at the jump, not after the body
    /// AND the error has the help that puts the jump in `( )`, except for a `break` that the `( )`
    /// would leave in a loop header (E0082) or outside every loop (E0038)
    /// AND a `continue`, which takes no value, and the same header with the jump in `( )` parse
    #[test]
    fn jump_that_takes_a_header_block_should_be_reported_at_the_jump() {
        let body = "{\n        f()\n    }";
        let rows = [
            ("if o ?? return", 13, true),
            ("if let Some(x) = o ?? return", 27, true),
            ("if c { f() } else if o ?? break", 31, false),
            ("while o ?? return", 16, true),
            ("while o ?? break", 16, false),
            ("while let Some(x) = o ?? break", 30, false),
            ("while let Some(x) = o ?? return", 30, true),
            ("for x in xs ?? return", 20, true),
            ("for x in xs ?? break", 20, false),
            ("for x in 0..n ?? break", 22, false),
            ("outer: while o ?? break outer", 23, false),
            ("match o ?? return", 16, true),
            ("match o ?? break", 16, false),
        ];
        for (header, column, help) in rows {
            let src = format!("fn main() {{\n    {header} {body}\n}}");
            assert_eq!(parse_err(&src), (Code::Expected, 2, column), "{src}");
            let found = parse_diagnostic(&src).help().is_some();
            assert_eq!(found, help, "{src}");
        }
        // A `match` has arms, not statements, in its body.
        let arms = "fn main() {\n    match o ?? return {\n        _ => f()\n    }\n}";
        assert_eq!(parse_err(arms), (Code::Expected, 2, 16));
        for header in [
            "while o ?? continue",
            "while o ?? continue outer",
            "for x in xs ?? continue",
            "if (o ?? return)",
            "for x in (xs ?? return)",
            "while (o ?? break)",
        ] {
            let src = format!("fn main() {{\n    {header} {body}\n}}");
            assert!(parsed(&src).diagnostics.is_empty(), "{src}");
        }
        assert!(
            parsed("fn main() {\n    match (o ?? return) {\n        _ => f()\n    }\n}")
                .diagnostics
                .is_empty()
        );
    }

    /// GIVEN a `match` header with a `return` or `break` whose value fails to parse, where the
    /// value does not start with `{`, or the `{` sits in a bracket that the header opened
    /// WHEN the parser reads it
    /// THEN the error is the plain one of the value, not the error of a jump that took the block
    #[test]
    fn jump_value_error_should_stay_plain_when_the_jump_took_no_block() {
        for jump in ["return", "break"] {
            // The value starts with `+`, not `{`, and the `{` after it is a block.
            let plus = format!("fn main() {{\n    match {jump} + {{\n        _ => f()\n    }}\n}}");
            // The `(` is open at the jump, so the block is inside it.
            let bracket = format!(
                "fn main() {{\n    match ({jump} {{\n        _ => f()\n    }}) {{\n        _ => f()\n    }}\n}}"
            );
            for (src, code, line) in [
                (plus, Code::Expected, 2),
                (bracket, Code::UnderscoreValue, 3),
            ] {
                let diagnostic = parse_diagnostic(&src);
                let (found, at, _) = parse_err(&src);
                assert_eq!((found, at), (code, line), "{src}");
                assert!(diagnostic.help().is_none(), "{src}");
            }
        }
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
    fn else_on_new_line_should_have_no_fix_after_anything_else() {
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
            // The `else` belongs to the `if` inside the `let`: the `if` wording, with no join.
            "fn main() {\n    let x = if a { 1 }\n    else { 2 }\n}",
            "fn main() {\n    let x = if a { 1 } else if b { 2 }\n    else { 3 }\n}",
            // A `let` value that ends in `}` would read the joined `else` as its own (E0173).
            "fn main() {\n    let x = { 1 }\n    else {\n    }\n}",
            // A typed `let` whose value ends in an `if` without `else`: the `else` is the `if`'s.
            // (A typed `let` with another value reports its type: E0011.)
            "fn main() {\n    let x: i64 = if a { 1 }\n    else { 2 }\n}",
            "fn main() {\n    let mut x: i64 = if a { 1 }\n    else { 2 }\n}",
            // A `let ... else` has no `else if`.
            "fn main() {\n    let x = o\n    else if a {\n    }\n}",
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::ElseOnNewLine, "{src}");
            assert_eq!(fix_edits(&d), None, "{src}");
        }
    }

    #[test]
    fn else_on_new_line_after_a_let_should_join_it_to_the_value() {
        // The gap from the end of the value to the `else` becomes one space.
        for src in [
            "fn main() {\n    let x = o\n    else { return }\n}",
            "fn main() {\n    let mut x = o\n    else { return }\n}",
            "fn main() {\n    let Some(x) = o\n    else { return }\n}",
            "fn main() {\n    let E.A = o\n    else { return }\n}",
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::ElseOnNewLine, "{src}");
            let else_at = src.find("else").expect("the source should hold an `else`");
            assert_eq!(d.span().start(), else_at, "{src}");
            let gap = src.find("= o").expect("the source should hold the value") + 3;
            assert_eq!(
                fix_edits(&d),
                Some((true, vec![(gap, else_at, " ".to_string())])),
                "{src}"
            );
        }
    }

    #[test]
    fn else_on_new_line_after_a_let_should_not_join_over_a_comment() {
        for src in [
            "fn main() {\n    let x = o // c\n    else { return }\n}",
            "fn main() {\n    let x = o\n    // c\n    else { return }\n}",
            "fn main() {\n    let E.A = o // c\n    else { return }\n}",
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::ElseOnNewLine, "{src}");
            assert!(fix_edits(&d).is_some_and(|(safe, _)| !safe), "{src}");
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
    fn new_tokens_should_be_errors_where_they_do_not_belong() {
        // `self` is an expression now, so its row hits a different error.
        for src in ["class", "is", "let self = 1", ".", "?", "?.", "??"] {
            assert_eq!(expr_err(src).0, Code::Expected, "{src}");
        }
    }

    /// `None` starts a pattern in a `let`. The parser reports nothing for a pattern with no
    /// `else`: the checker decides by the value's type.
    #[test]
    fn let_none_pattern_without_else_should_parse_clean() {
        let parsed = parsed("fn main() {\n    let None = 1\n}");
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        assert!(parsed.checkable);
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
    fn assignment_through_safe_access_should_parse() {
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
            parse_ok(&format!("fn main() {{ {src} }}"));
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
    fn alias_items_should_parse_to_alias_items() {
        for (src, want) in [
            ("type Grid = [[char?]]", "(type Grid [[char?]])"),
            ("type Pairs<T> = [[T]]", "(type Pairs<T> [[T]])"),
            ("type M<K, V> = [K]", "(type M<K, V> [K])"),
            // A trailing comma ends the list, as it does for a function.
            ("type M<K, V,> = [K]", "(type M<K, V> [K])"),
            // An alias is one line, so the item after it starts on the next.
            (
                "type A = i64\nfn main() {}",
                "(type A i64)\n(fn main () unit (block))",
            ),
            // An alias may name itself or the type that follows: the checker reports cycles.
            ("type A = A?", "(type A A?)"),
        ] {
            assert_eq!(program(src), want, "{src}");
        }
        for (src, code, line, col) in [
            // An alias's parameters take no bounds.
            ("type S<T: Ord> = [T]", Code::Expected, 1, 9),
            ("type A", Code::Expected, 1, 7),
            ("type A =", Code::Expected, 1, 9),
            ("type A = i64 x", Code::Expected, 1, 14),
            ("type A<> = i64", Code::Expected, 1, 8),
            ("type _ = i64", Code::UnderscoreName, 1, 6),
            ("type A<_> = i64", Code::UnderscoreName, 1, 8),
            // An alias is an item, so a function body has none.
            ("fn f() { type A = i64 }", Code::Expected, 1, 10),
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

    /// Only the prelude may declare a variant `None`, and so name `Option`'s.
    #[test]
    fn none_variant_should_parse_only_in_the_prelude() {
        let src = "enum Option<T> {\n    None\n    Some(T)\n}";
        assert_eq!(
            parse(lex(src).expect("source lexes"), true)
                .program
                .to_string(),
            "(enum Option<T> None (Some T))"
        );
        assert_eq!(parse_err(src), (Code::BuiltInName, 2, 5));
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
    fn unnamed_item_with_dangling_operator_should_end_at_the_next_function() {
        let parsed = parsed("let y = 1 +\nfn main() {\n}");
        assert_eq!(parsed.diagnostics.len(), 1, "{:?}", parsed.diagnostics);
        assert!(!parsed.checkable);
        assert_eq!(parsed.program.to_string(), "(fn main () unit (block))");
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
    fn generic_declarations_should_parse() {
        let rows = [
            (
                "fn first<T>(xs: [T]) -> T? {}",
                "(fn first<T> ((xs [T])) T? (block))",
            ),
            (
                "fn pair<K, V,>(k: K, v: V) {}",
                "(fn pair<K, V> ((k K) (v V)) unit (block))",
            ),
            (
                "class Stack<T> {\n    mut items: [T]\n}",
                "(class Stack<T> (mut items [T]))",
            ),
            (
                "enum Tree<T> {\n    Leaf\n    Node(Tree<T>, T, Tree<T>)\n}",
                "(enum Tree<T> Leaf (Node Tree<T> T Tree<T>))",
            ),
            (
                "class Stack<T> {\n    fn map_all<U>(self, f: U) {}\n}",
                "(class Stack<T> (fn map_all<U> (self (f U)) unit (block)))",
            ),
            ("const c: Stack<i64>= x", "(const c Stack<i64> x)"),
        ];
        for (src, expected) in rows {
            assert_eq!(program(src), expected, "{src}");
        }
    }

    #[test]
    fn a_bad_type_parameter_list_should_be_rejected() {
        assert_eq!(parse_err("fn f<>() {}"), (Code::Expected, 1, 6));
        assert_eq!(parse_err("class A<_> {}"), (Code::UnderscoreName, 1, 9));
        // A bound is a type, so a list that ends before it is missing a type.
        assert_eq!(parse_err("fn f<T: >() {}"), (Code::Expected, 1, 9));
        assert_eq!(parse_err("fn f<T: A &>() {}"), (Code::Expected, 1, 12));
        assert_eq!(parse_err("fn f<T: A B>() {}"), (Code::Expected, 1, 11));
    }

    /// Each row is the `--emit ast` text of a declaration with a bound, a conformance list, an
    /// associated type or an `extend` block.
    #[test]
    fn interfaces_extend_and_bounds_should_parse() {
        let rows = [
            ("fn f<T: Ord>() {}", "(fn f<T: Ord> () unit (block))"),
            ("enum E<T: Ord> {}", "(enum E<T: Ord>)"),
            (
                "fn to<T: Convert<Kelvin>>(x: T) {}",
                "(fn to<T: Convert<Kelvin>> ((x T)) unit (block))",
            ),
            (
                "fn f<T: Shape & Same, U>(x: T, y: U.Item, z: [S.Item.Key]?) {}",
                "(fn f<T: Shape & Same, U> ((x T) (y U.Item) (z [S.Item.Key]?)) unit (block))",
            ),
            (
                "interface Shape: Display {\n    fn area(self) -> f64\n}",
                "(interface Shape (: Display) (fn area (self) f64))",
            ),
            (
                "interface Convert<T> { fn convert(self) -> T }",
                "(interface Convert<T> (fn convert (self) T))",
            ),
            (
                "interface Source {\n    type Item: Same & Named\n    fn next(self) -> Item?\n}",
                "(interface Source (type Item: Same & Named) (fn next (self) Item?))",
            ),
            (
                "interface Plain {\n    type Item\n    fn map<U: Same>(self, f: U) -> Self\n}",
                "(interface Plain (type Item) (fn map<U: Same> (self (f U)) Self))",
            ),
            ("interface Marker {}", "(interface Marker)"),
            (
                "class Circle: Shape, Same {\n    r: f64\n}",
                "(class Circle (: Shape Same) (r f64))",
            ),
            (
                "enum E: Same {\n    A\n    fn same(self, other: Self) -> bool { true }\n}",
                "(enum E (: Same) A (fn same (self (other Self)) bool (block true)))",
            ),
            (
                "class Countdown: Source {\n    type Item = i64\n    n: i64\n}",
                "(class Countdown (: Source) (type Item i64) (n i64))",
            ),
            (
                "class C {\n    fn f(self, a: Self, b: [Self], c: Self?) -> Self? {}\n}",
                "(class C (fn f (self (a Self) (b [Self]) (c Self?)) Self? (block)))",
            ),
            (
                "extend Circle {\n    fn grow(self) {}\n}",
                "(extend Circle (fn grow (self) unit (block)))",
            ),
            (
                "extend Celsius: Convert<Kelvin> {\n    fn convert(self) -> Kelvin {}\n}",
                "(extend Celsius: Convert<Kelvin> (fn convert (self) Kelvin (block)))",
            ),
            (
                "extend Wrap: Source {\n    type Item = i64\n}",
                "(extend Wrap: Source (type Item i64))",
            ),
            (
                "extend<T: Same> Stack<T>: Same {\n    fn same(self, other: Self) -> bool { true }\n}",
                "(extend<T: Same> Stack<T>: Same (fn same (self (other Self)) bool (block true)))",
            ),
            ("extend Stack<i64> {}", "(extend Stack<i64>)"),
            // The checker, not the parser, says that these cannot be extended.
            (
                "enum E: Source {\n    A\n    type Item = i64\n}",
                "(enum E (: Source) (type Item i64) A)",
            ),
            ("extend [i64] {}", "(extend [i64])"),
            ("extend<T> T {}", "(extend<T> T)"),
        ];
        for (src, expected) in rows {
            assert_eq!(program(src), expected, "{src}");
        }
    }

    /// A `;` between two members of an interface or an `extend` block separates them, as in a
    /// class (ADR 0028), and one after the item is redundant.
    #[test]
    fn a_semicolon_in_an_interface_or_extend_block_should_end_a_member() {
        for (line, same_line) in [
            (
                "interface Shape {\n    fn area(self) -> f64\n    fn name(self) -> string\n}",
                "interface Shape { fn area(self) -> f64; fn name(self) -> string }",
            ),
            (
                "interface Source {\n    type Item\n    fn next(self) -> Item?\n}",
                "interface Source { type Item; fn next(self) -> Item? }",
            ),
            (
                "extend Circle {\n    fn a(self) {}\n    fn b(self) {}\n}",
                "extend Circle { fn a(self) {}; fn b(self) {} }",
            ),
        ] {
            assert_eq!(program(same_line), program(line), "{same_line}");
        }
        // The words before `fn` stand on one line, so only `between_items` makes the `;` redundant.
        for src in [
            "interface I {};",
            "extend C {};",
            "interface I {}; fn main() {}",
            "extend C {}; fn main() {}",
        ] {
            let ds = parsed(src).diagnostics;
            assert_eq!(
                ds.iter().map(Diagnostic::code).collect::<Vec<_>>(),
                [Code::RedundantSemicolon],
                "{src}"
            );
            assert!(fix_edits(&ds[0]).is_some_and(|(safe, _)| safe), "{src}");
        }
    }

    /// Each row is the first parse error of a declaration the grammar of interfaces rejects.
    #[test]
    fn a_malformed_interface_or_extend_block_should_be_rejected() {
        for (src, want) in [
            // A body after a requirement, and a field.
            (
                "interface I {\n    fn f(self) -> i64 { 1 }\n}",
                (Code::InterfaceBody, 2, 23),
            ),
            ("interface I {\n    x: i64\n}", (Code::InterfaceBody, 2, 5)),
            (
                "interface I {\n    mut x: i64\n}",
                (Code::InterfaceBody, 2, 5),
            ),
            // A requirement takes `self`, and never `mut self`.
            (
                "interface I {\n    fn f()\n}",
                (Code::MethodNeedsSelf, 2, 8),
            ),
            (
                "interface I {\n    fn f(mut self)\n}",
                (Code::MutSelf, 2, 10),
            ),
            ("interface I {\n    1\n}", (Code::Expected, 2, 5)),
            // `type` is a keyword: it is no name, and it starts an item or a choice only where the
            // grammar reads one.
            ("fn main() {\n    let type = 1\n}", (Code::Expected, 2, 9)),
            ("fn main() {\n    print(p.type)\n}", (Code::Expected, 2, 13)),
            (
                "extend Circle {\n    type Item = i64\n}",
                (Code::Expected, 2, 5),
            ),
            ("class C {\n    type Item = i64\n}", (Code::Expected, 2, 5)),
            // `extend` and `Self` are keywords, not names.
            ("fn main() {\n    let extend = 1\n}", (Code::Expected, 2, 9)),
            (
                "fn main() {\n    let interface = 1\n}",
                (Code::Expected, 2, 9),
            ),
            ("class Self {}", (Code::Expected, 1, 7)),
            (
                "class C {\n    fn f(self) {\n        Self(r: 1)\n    }\n}",
                (Code::Expected, 3, 9),
            ),
            // Nothing follows an `extend` target but `:` and `{`.
            ("extend C: A, B {}", (Code::Expected, 1, 12)),
            ("extend C {\n    x: i64\n}", (Code::Expected, 2, 5)),
            ("extend<> C {}", (Code::Expected, 1, 8)),
            (
                "extend C {\n    fn f(self) {} x\n}",
                (Code::Expected, 2, 19),
            ),
            ("extend C: I {\n    type Item\n}", (Code::Expected, 2, 14)),
            (
                "extend C {\n    fn f(mut self: ) {}\n}",
                (Code::Expected, 2, 20),
            ),
            // The pieces of an interface and of an associated type.
            ("interface {}", (Code::Expected, 1, 11)),
            ("interface I<> {}", (Code::Expected, 1, 13)),
            ("interface I x {}", (Code::Expected, 1, 13)),
            ("interface I {\n    type 1\n}", (Code::Expected, 2, 10)),
            // A `:` cannot end a line, so the type is missing where the next line starts.
            ("interface I {\n    type Item:\n}", (Code::Expected, 3, 1)),
            ("interface I {\n    x i64\n}", (Code::Expected, 2, 7)),
            (
                "interface I {\n    fn f(self) x\n}",
                (Code::Expected, 2, 16),
            ),
            ("class C: {}", (Code::Expected, 1, 10)),
            ("enum E: {}", (Code::Expected, 1, 9)),
            // Neither `type` nor `=` ends a line, so what is missing is found on the next one.
            ("class C: S {\n    type\n}", (Code::Expected, 3, 1)),
            ("class C: S {\n    type Item\n}", (Code::Expected, 2, 14)),
            ("class C: S {\n    type Item =\n}", (Code::Expected, 3, 1)),
            (
                "class C: S {\n    type Item = i64 x\n}",
                (Code::Expected, 2, 21),
            ),
            ("enum E: S {\n    type Item\n}", (Code::Expected, 2, 14)),
            // Only an enum that lists an interface chooses an associated type.
            (
                "enum E {\n    A\n    type Item = i64\n}",
                (Code::Expected, 3, 5),
            ),
            ("fn f(x: S.) {}", (Code::Expected, 1, 10)),
        ] {
            assert_eq!(parse_err(src), want, "{src}");
        }
    }

    /// A lone `&` is a token for a bound's list only: anywhere else it is E0010 at the `&`. Only
    /// an operator position can mean `&&`, so none of these rows offers a fix. The rows are an
    /// operand, an item, a binding, a pattern, a loop, a type, a bound and an assignment, each a
    /// different "expected" path.
    #[test]
    fn a_lone_amp_outside_an_operator_position_should_be_an_unexpected_character() {
        for (src, at) in [
            ("fn main() {\n    print(&a)\n}", (2, 11)),
            ("&\nfn main() {}", (1, 1)),
            ("fn main() {\n    let & = 1\n}", (2, 9)),
            ("fn main() {\n    let x & = 1\n}", (2, 11)),
            ("fn f(x: A & B) {}\nfn main() {}", (1, 11)),
            ("fn main() {\n    for & in [1] {}\n}", (2, 9)),
            ("fn main() {\n    match 1 { & => 1 }\n}", (2, 15)),
            ("fn main() {\n    let a = 1\n    a &= 2\n}", (3, 7)),
            // A line start is no operator position: the line break ends the statement.
            ("fn main() {\n    let a = 1\n    & 2\n}", (3, 5)),
            // The `{` after the `&` opens the body, so no operand follows the `&`.
            (
                "fn main() {\n    if a & {\n        print(1)\n    }\n}",
                (2, 10),
            ),
            (
                "fn main() {\n    while a & {\n        print(1)\n    }\n}",
                (2, 13),
            ),
            (
                "fn main() {\n    for x in [1] & {\n        print(x)\n    }\n}",
                (2, 18),
            ),
            (
                "fn main() {\n    match a & {\n        _ => 1\n    }\n}",
                (2, 13),
            ),
            (
                "fn main() {\n    if let x = a & {\n        print(x)\n    }\n}",
                (2, 18),
            ),
            (
                "fn main() {\n    if a {\n    } else if b & {\n    }\n}",
                (3, 17),
            ),
            // No operand follows the `&`.
            ("fn main() {\n    print(1 & )\n}", (2, 13)),
            ("fn main() {\n    let a = 1 &\n}", (2, 15)),
            ("fn main() {\n    print(1 & _)\n}", (2, 13)),
            // A literal that `&&` would not take either.
            (
                "fn main() {\n    print(1 & 9223372036854775808)\n}",
                (2, 13),
            ),
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::UnexpectedCharacter, "{src}");
            assert_eq!(line_col(src, d.span().start()), at, "{src}");
            assert_eq!(fix_edits(&d), None, "{src}");
        }
    }

    /// A lone `|` outside an operator position is E0010 with no fix, as a lone `&` is.
    #[test]
    fn a_lone_pipe_outside_an_operator_position_should_be_an_unexpected_character() {
        for (src, at) in [
            ("fn main() {\n    print(|a)\n}", (2, 11)),
            ("fn main() {\n    let a = |1\n}", (2, 13)),
            ("fn main() {\n    print(1 | )\n}", (2, 13)),
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::UnexpectedCharacter, "{src}");
            assert_eq!(line_col(src, d.span().start()), at, "{src}");
            assert_eq!(fix_edits(&d), None, "{src}");
        }
    }

    /// A lone `&` or `|` after a complete operand, with an operand after it, is most likely `&&`
    /// or `||`. The fix is unsafe, and with it applied the source parses.
    #[test]
    fn a_lone_amp_or_pipe_between_operands_should_offer_the_logical_operator() {
        for (src, at, edit, with) in [
            ("fn main() {\n    print(a & b)\n}", (2, 13), (24, 25), "&&"),
            ("fn main() {\n    print(a | b)\n}", (2, 13), (24, 25), "||"),
            ("fn main() {\n    if a & b {}\n}", (2, 10), (21, 22), "&&"),
            ("fn main() {\n    return a | b\n}", (2, 14), (25, 26), "||"),
            (
                "fn main() {\n    let c = a < b & !c\n}",
                (2, 19),
                (30, 31),
                "&&",
            ),
            (
                "fn main() {\n    let c = a + b | -c\n}",
                (2, 19),
                (30, 31),
                "||",
            ),
            // The operand is on the next line: the `&` ends no statement.
            (
                "fn main() {\n    let c = a &\n        b\n}",
                (2, 15),
                (26, 27),
                "&&",
            ),
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::UnexpectedCharacter, "{src}");
            assert_eq!(line_col(src, d.span().start()), at, "{src}");
            assert_eq!(
                fix_edits(&d),
                Some((false, vec![(edit.0, edit.1, with.to_string())])),
                "{src}"
            );
            assert!(parsed(&apply_fix(src, &d)).diagnostics.is_empty(), "{src}");
        }
    }

    /// A broken `extend` block is named by its target when that is a name, and the parser goes
    /// on with the next item; with no name to give, the file is not `checkable`.
    #[test]
    fn a_broken_extend_block_should_be_named_by_its_target() {
        for (src, name) in [
            ("extend C: {}\nfn main() {}", "C"),
            ("extend<T> Stack<T>: {}\nfn main() {}", "Stack"),
            (
                "extend<T: Same & Convert<K>> Stack<T>: {}\nfn main() {}",
                "Stack",
            ),
        ] {
            let parsed = parsed(src);
            let named: Vec<_> = (parsed.program.broken.iter())
                .map(|b| (b.name.name.as_str(), b.kind))
                .collect();
            assert_eq!(named, [(name, BrokenKind::Extend)], "{src}");
            assert!(parsed.checkable, "{src}");
            assert_eq!(parsed.diagnostics.len(), 1, "{src}");
            assert_eq!(parsed.program.items.len(), 1, "{src}");
        }
        // A type-parameter list that is not closed on the line leaves no name to give.
        for src in [
            "extend {}\nfn main() {}",
            "extend [i64]: {}\nfn main() {}",
            "extend<T {}\nfn main() {}",
            "extend<T\nfn main() {}",
        ] {
            let parsed = parsed(src);
            assert!(parsed.program.broken.is_empty(), "{src}");
            assert!(!parsed.checkable, "{src}");
            assert_eq!(parsed.diagnostics.len(), 1, "{src}");
            assert_eq!(parsed.program.items.len(), 1, "{src}");
        }
        // The last item: the scan meets `Eof`, a `{` or a line break, not another item.
        for src in ["extend<", "extend<T {}", "extend<T\n"] {
            let parsed = parsed(src);
            assert!(parsed.program.broken.is_empty(), "{src}");
            assert!(!parsed.checkable, "{src}");
            assert_eq!(parsed.diagnostics.len(), 1, "{src}");
        }
    }

    /// The scan for an `extend` target stops at the next item line. An open `<` that no `>` closes
    /// before it leaves the block unnamed, so each `extend<` line costs its own tokens and not the
    /// rest of the file. The `>` after the second `extend` must not close the first one's list.
    #[test]
    fn the_scan_for_an_extend_target_should_stop_at_the_next_item_line() {
        let parsed = parsed("extend<\nextend C> D {}\nfn main() {}");
        let named: Vec<_> = (parsed.program.broken.iter())
            .map(|b| b.name.name.as_str())
            .collect();
        assert_eq!(named, ["C"]);
        assert!(!parsed.checkable);
    }

    /// A method that holds an error is broken alone: its interface or `extend` block stays
    /// intact, with the methods before and after it, and names the lost one.
    #[test]
    fn a_broken_method_should_not_break_its_interface_or_extend_block() {
        for (src, intact) in [
            ("interface I {\n    fn a(self) {}\n    fn b(self)\n}", 1),
            (
                "extend C {\n    fn a(self) {\n        let = 1\n    }\n    fn b(self) {}\n}",
                1,
            ),
        ] {
            let parsed = parsed(src);
            assert_eq!(parsed.diagnostics.len(), 1, "{src}");
            assert!(parsed.program.broken.is_empty(), "{src}");
            let (methods, lost) = match &parsed.program.items[..] {
                [Item::Interface(i)] => (i.methods.len(), &i.broken_methods),
                [Item::Extend(e)] => (e.methods.len(), &e.broken_methods),
                other => panic!("{src}: expected one item, found {other:?}"),
            };
            assert_eq!(methods, intact, "{src}");
            let lost: Vec<_> = lost.iter().map(|name| name.name.as_str()).collect();
            assert_eq!(lost, ["a"], "{src}");
        }
    }

    /// A broken `interface` header keeps its name, and the item after it is read.
    #[test]
    fn a_broken_interface_header_should_keep_its_name() {
        let parsed = parsed("interface Shape: {}\nfn main() {}");
        let named: Vec<_> = (parsed.program.broken.iter())
            .map(|b| (b.name.name.as_str(), b.kind))
            .collect();
        assert_eq!(named, [("Shape", BrokenKind::Interface)]);
        assert!(parsed.checkable);
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.program.items.len(), 1);
    }

    /// An item that no line break ends yet starts at `interface` or `extend` at a line start, so
    /// an error before it and one in it are both reported.
    #[test]
    fn interface_and_extend_should_end_the_item_before_them() {
        for (src, codes) in [
            (
                "let x = 1\ninterface I {\n    fn f(self) {}\n}",
                [Code::Expected, Code::InterfaceBody],
            ),
            (
                "let x = 1\nextend C {\n    type Item = i64\n}",
                [Code::Expected, Code::Expected],
            ),
            (
                "fn f() +\ninterface I: {}",
                [Code::Expected, Code::Expected],
            ),
            ("fn f() +\nextend C: {}", [Code::Expected, Code::Expected]),
        ] {
            let found: Vec<_> = (parsed(src).diagnostics.iter())
                .map(Diagnostic::code)
                .collect();
            assert_eq!(found, codes, "{src}");
        }
    }

    /// In an `extend` block the parser cannot tell a class from an enum, so it records `mut self`
    /// for the checker, with the `: Type` that makes its fix unsafe.
    #[test]
    fn mut_self_in_an_extend_block_should_be_recorded_not_reported() {
        let rows = [
            (
                "extend Circle {\n    fn area(mut self) {}\n}",
                28..31,
                32..36,
                None,
            ),
            (
                "extend Circle {\n    fn area(mut self, n: i64) {}\n}",
                28..31,
                32..36,
                None,
            ),
            (
                "extend Circle {\n    fn area(mut self: i64) {}\n}",
                28..31,
                32..36,
                Some(36..41),
            ),
        ];
        for (src, keyword, receiver, annotation) in rows {
            let Some(Item::Extend(extend)) = parse_ok(src).items.pop() else {
                panic!("{src}: expected an `extend` block");
            };
            let span = |r: Range<usize>| Span::new(r.start, r.end);
            let want = MutSelf {
                keyword: span(keyword),
                receiver: span(receiver.clone()),
                annotation: annotation.map(span),
            };
            assert_eq!(extend.methods[0].mut_self, Some(want), "{src}");
            assert_eq!(extend.methods[0].self_param, Some(span(receiver)), "{src}");
        }
        // A class method has no record: its `mut self` is an error at once.
        assert_eq!(
            parse_err("class C {\n    fn f(mut self) {}\n}"),
            (Code::MutSelf, 2, 10)
        );
    }

    #[test]
    fn generic_types_should_parse() {
        let rows = [
            ("Stack<i64>", "Stack<i64>"),
            ("[Stack<i64>]", "[Stack<i64>]"),
            ("Stack<i64>?", "Stack<i64>?"),
            ("Map<string, [i64]>", "Map<string, [i64]>"),
            ("Map<string, [i64],>", "Map<string, [i64]>"),
            ("Stack<Stack<i64>>", "Stack<Stack<i64>>"),
        ];
        for (ty, expected) in rows {
            let src = format!("fn main() {{ let x: {ty} = y }}");
            assert_eq!(body(&src), format!("(block (let x {expected} y))"), "{src}");
        }
        // A type argument is a type: a number is none.
        assert_eq!(parse_err("fn f(x: Stack<1>) {}"), (Code::Expected, 1, 15));
    }

    /// `>=` closes the list that ends before it, and the `=` stays for the `let`.
    #[test]
    fn a_greater_equal_should_close_a_type_argument_list() {
        for (src, expected) in [
            ("let s: Stack<i64>= x", "(let s Stack<i64> x)"),
            ("let t: A<B<i64>>= y", "(let t A<B<i64>> y)"),
            ("let t: A<B<i64>,>= y", "(let t A<B<i64>> y)"),
        ] {
            assert_eq!(expr(src), expected, "{src}");
        }
        // The type ends with its `>`, not with the `>=`.
        let src = "fn main() { let s: Stack<i64>= x }";
        let StmtKind::Let { ty: Some(ty), .. } = &first_fn(src).body.stmts[0].kind else {
            panic!("expected a typed let");
        };
        assert_eq!(&src[ty.span.range()], "Stack<i64>");
        // One `>=` closes one list, so the outer list is left open, and what is left of the
        // token is the `=` after the `>`, wherever an error finds it.
        assert_eq!(expr_err("let t: A<B<i64>= y"), (Code::Expected, 1, 28));
        assert_eq!(expr_err("let t: [A<i64>= y] = z"), (Code::Expected, 1, 27));
        assert_eq!(parse_err("fn f(x: A<i64>= 1) {}"), (Code::Expected, 1, 15));
    }

    /// A line break after the `>` that closes type arguments ends a statement, as one after a
    /// type with none does; inside `( )` it does not, and after a `>` that compares, the line goes
    /// on.
    #[test]
    fn a_line_break_after_type_arguments_should_end_the_statement() {
        for (src, expected) in [
            (
                "class D {\n    b: Box<i64>\n    n: i64\n}",
                "(class D (b Box<i64>) (n i64))",
            ),
            (
                "fn f(\n    a: Box<i64>\n) {}",
                "(fn f ((a Box<i64>)) unit (block))",
            ),
            (
                "fn f(a: Box<i64>,\n    b: i64) {}",
                "(fn f ((a Box<i64>) (b i64)) unit (block))",
            ),
            (
                "fn f() {\n    let t = Tree<i64>\n        .Leaf\n}",
                "(fn f () unit (block (let t (. Tree<i64> Leaf))))",
            ),
            (
                "fn f() {\n    let b = a >\n        c\n    let d = a >\n        - c\n    let e = (a >\n        c)\n}",
                "(fn f () unit (block (let b (> a c)) (let d (> a (- c))) (let e (> a c))))",
            ),
            // A `{` after a comparison's `>` is its operand, outside parentheses and inside.
            (
                "fn f() {\n    let b = 2 >\n        { 1 }\n    print(2 >\n        { 1 })\n}",
                "(fn f () unit (block (let b (> 2 (block 1))) (call print (> 2 (block 1)))))",
            ),
            // A function's header goes on after its type parameters.
            ("fn f<T>\n(x: T) {}", "(fn f<T> ((x T)) unit (block))"),
            (
                "class C {\n    fn m<U>\n    (self, y: U) {}\n}",
                "(class C (fn m<U> (self (y U)) unit (block)))",
            ),
            // So does a call's line after its type arguments, though a `:` comes before them,
            // and a comparison's, though one is inside its parentheses.
            (
                "fn f() {\n    let g: i64 = f<i64>\n    (x)\n    let b = g(x: a < c) >\n        d\n}",
                "(fn f () unit (block (let g i64 (call f<i64> x)) (let b (> (call g (: x (< a c))) d))))",
            ),
            // A constant's comparison goes on too.
            ("const b = 2 >\n    1", "(const b (> 2 1))"),
            // A type with an inner list closed at a line's end goes on to its outer `>`.
            (
                "class D {\n    b: Box<Box<i64>\n    >\n    n: i64\n}",
                "(class D (b Box<Box<i64>>) (n i64))",
            ),
            // A `>=` that closes a type annotation holds its `=`, so a comparison in the value
            // after it goes on, fused, spaced and nested alike.
            (
                "fn f() {\n    let x: Box<i64>= if 2 >\n        1 { y } else { z }\n}",
                "(fn f () unit (block (let x Box<i64> (if (> 2 1) (block y) (block z)))))",
            ),
            (
                "fn f() {\n    let x: Box<i64> = if 2 >\n        1 { y } else { z }\n}",
                "(fn f () unit (block (let x Box<i64> (if (> 2 1) (block y) (block z)))))",
            ),
            (
                "fn f() {\n    let x: Box<Box<i64>>= if 2 >\n        1 { y } else { z }\n}",
                "(fn f () unit (block (let x Box<Box<i64>> (if (> 2 1) (block y) (block z)))))",
            ),
        ] {
            assert_eq!(program(src), expected, "{src}");
        }
        // A header's condition that ends in `>` takes the `{` as its operand, so the error is
        // the missing body, with no join to offer.
        for src in [
            "fn f() {\n    if a >\n    {\n    }\n}",
            "fn f() {\n    while a >\n    {\n    }\n}",
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::Expected, "{src}");
            assert!(fix_edits(&d).is_none(), "{src}");
        }
        assert_eq!(parse_err("fn f<T>; (x: T) {}"), (Code::Expected, 1, 8));
        // An error in a split header's body is the only one: the header holds together.
        assert_eq!(
            parse_err("fn f<T>\n(x: T) { let = 1 }"),
            (Code::Expected, 2, 14)
        );
        // Without type parameters, the line break ends the header, as it always has.
        let d = &parsed("fn f\n(x: i64) {}").diagnostics[0];
        assert_eq!((d.code(), d.span().start()), (Code::Expected, 4));
        // As after `i64`: the `=` cannot start the next line, nor the `{`.
        assert_eq!(
            parse_err("fn f() {\n    let s: Box<i64>\n    = x\n}"),
            (Code::Expected, 2, 20)
        );
    }

    /// A line break in a header's type-parameter list goes on only before a name, `,` or `>`,
    /// so a list that is not closed or holds another token reports as before, and a `<` that
    /// compares still compares.
    #[test]
    fn a_malformed_type_parameter_list_should_keep_its_diagnostic() {
        // The `{` left open stops the lexer, with its one diagnostic.
        for src in ["fn f<T\n{", "fn f<T>\n{"] {
            let d = lex(src).expect_err("the open `{` should stop the lexer");
            let (line, col) = line_col(src, d.span().start());
            assert_eq!((d.code(), line, col), (Code::BraceOnNewLine, 2, 1), "{src}");
        }
        for (src, expected) in [
            ("fn f<T, 1>(x: T) {}", (Code::Expected, 1, 9)),
            (
                "fn main() {\n    let a = P<\n i64\n>(1)\n}",
                (Code::Expected, 4, 1),
            ),
            // A keyword is no name: it starts the next item.
            ("fn f<T\nfn g() {}", (Code::Expected, 1, 7)),
        ] {
            assert_eq!(parse_err(src), expected, "{src}");
        }
        // Only the `<` after the header's name opens the list, not the one of a return type.
        let src = "fn f() -> Box<\n    i64\n> {}";
        let found: Vec<_> = (parsed(src).diagnostics.iter())
            .map(|d| (d.code(), line_col(src, d.span().start())))
            .collect();
        assert_eq!(found, [(Code::Expected, (2, 8)), (Code::Expected, (3, 1))]);
        // Nor a `<` that is the third token of another statement.
        assert_eq!(
            program("fn f() -> bool {\n    return a <\n        b\n    print(1)\n}"),
            "(fn f () bool (block (return (< a b)) (call print 1)))"
        );
    }

    /// The name on the next line goes on with a method's list that is not closed, so the one
    /// error is at that name.
    #[test]
    fn an_unclosed_method_list_should_report_the_name_on_the_next_line() {
        assert_eq!(
            parse_err("class C {\n    fn m<T\n    x: i64\n}"),
            (Code::Expected, 3, 5)
        );
    }

    /// A header's line that ends in type arguments ends the header, so a `{` on the next line
    /// is E0004, as after a type with none.
    #[test]
    fn a_brace_after_a_header_ending_in_type_arguments_should_be_e0004() {
        assert_eq!(
            parse_err("fn f() -> Box<i64>\n{\n}"),
            (Code::BraceOnNewLine, 2, 1)
        );
        // A return type at a line's end ends the header, as `-> i64` does.
        assert_eq!(
            parse_err("fn f() -> Box<i64>\nfn g() {}"),
            (Code::Expected, 1, 19)
        );
        // After another statement too, ended by a line break or a `;`: the header is its own.
        assert_eq!(
            parse_err("const a = 1\nfn f() -> Box<i64>\n{\n}"),
            (Code::BraceOnNewLine, 3, 1)
        );
        assert_eq!(
            parse_err("const a = 1; fn f() -> Box<i64>\n{\n}"),
            (Code::BraceOnNewLine, 2, 1)
        );
        // Inside the header's parentheses too: the statement around them is still the header.
        assert_eq!(
            parse_err("fn f(x: Box<i64>\n{}) {}"),
            (Code::BraceOnNewLine, 2, 1)
        );
        // A split header's brace on its own line gets that error alone, in a function and a
        // method.
        assert_eq!(
            parse_err("fn f<T>\n(x: T) -> Box<T>\n{\n}"),
            (Code::BraceOnNewLine, 3, 1)
        );
        assert_eq!(
            parse_err("class C {\n    fn m<U>\n    (self, y: U) -> Box<U>\n    {\n    }\n}"),
            (Code::BraceOnNewLine, 4, 5)
        );
    }

    #[test]
    fn type_arguments_in_expressions_should_parse() {
        let rows = [
            ("f<i64>(x)", "(call f<i64> x)"),
            ("f<i64,>(x)", "(call f<i64> x)"),
            ("f<i64?>(x)", "(call f<i64?> x)"),
            ("f<Stack<i64>>(x)", "(call f<Stack<i64>> x)"),
            (
                "Stack<string>(items: [])",
                "(call Stack<string> (: items (array)))",
            ),
            ("Tree<i64>.Leaf", "(. Tree<i64> Leaf)"),
            ("Tree<i64>.Node(x)", "(call (. Tree<i64> Node) x)"),
            ("Option.None", "(. Option None)"),
            ("Option<i64>.None", "(. Option<i64> None)"),
            ("Option.Some(x)", "(call (. Option Some) x)"),
            ("s.map_all<f64>(1.5)", "(call (. s map_all<f64>) 1.5)"),
            ("largest<[i64]>([])", "(call largest<[i64]> (array))"),
            ("a < b > (c)", "(call a<b> c)"),
            ("f(a < b, c > (d))", "(call f (call a<b, c> d))"),
        ];
        for (src, expected) in rows {
            assert_eq!(expr(src), expected, "{src}");
        }
    }

    /// A `<` that no type-argument list and `(` or `.` completes is a comparison.
    #[test]
    fn a_less_than_should_be_a_comparison_unless_type_arguments_follow() {
        let rows = [
            ("f(a < b, c > d)", "(call f (< a b) (> c d))"),
            ("a < b", "(< a b)"),
            ("x < y.len()", "(< x (call (. y len)))"),
            ("while i < n { }", "(while (< i n) (block))"),
        ];
        for (src, expected) in rows {
            assert_eq!(expr(src), expected, "{src}");
        }
    }

    #[test]
    fn a_non_list_after_a_less_than_should_keep_its_diagnostic() {
        for (src, code, col) in [
            ("a < b > c", Code::ComparisonDirection, 19),
            ("a < [b] > c", Code::ComparisonDirection, 21),
            ("a < [b, c] > (d)", Code::ComparisonDirection, 24),
            ("a < b c > (d)", Code::Expected, 19),
            ("a < [] > (d)", Code::ComparisonDirection, 20),
            // A field takes no type arguments, so there is nowhere to put these.
            ("a.b<i64>.c", Code::Expected, 21),
        ] {
            assert_eq!(expr_err(src), (code, 1, col), "{src}");
        }
    }

    /// Runs `f` on a parser of `src` at token `pos`.
    fn parser_at<R>(src: &str, pos: usize, f: impl FnOnce(&mut Parser<'_>) -> R) -> R {
        let lexed = lex(src).expect("source lexes");
        f(&mut Parser {
            source: lexed.source,
            tokens: &lexed.tokens,
            pos,
            depth: Height::default(),
            lexer: &lexed.diagnostics,
            comments: &lexed.comments,
            breaks: &lexed.breaks,
            recovered: Vec::new(),
            header: false,
            pattern_place: PatternPlace::Arm,
            warnings: Vec::new(),
            labels: Vec::new(),
            loops: Vec::new(),
            prelude: false,
            struct_self: false,
            window: &lexed.tokens,
            halves: &OnceCell::new(),
            halved: None,
            scrutinee: None,
        })
    }

    /// Whether `type_args_follow` holds at the `<` that is the second token of `src`.
    fn follows(src: &str) -> bool {
        parser_at(src, 1, |p| p.type_args_follow())
    }

    #[test]
    fn last_span_after_a_skip_past_the_window_should_be_the_whole_token() {
        // Error recovery moves `pos` past a window that a split cut down.
        let (span, token) = parser_at("fn f() {}\nfn g() {}", 0, |p| {
            p.window = &p.tokens[..2];
            p.pos = 4;
            let token = p.bump().span;
            (p.last_span(), token)
        });
        assert_eq!(span, token);
    }

    #[test]
    fn last_span_after_a_split_greater_equal_should_be_the_equals_half() {
        let span = parser_at("A<i64>= x", 0, |p| {
            p.parse_type().expect("the type parses");
            p.expect(&TokenKind::Eq).expect("the `=` half is left");
            p.last_span()
        });
        assert_eq!((span.start(), span.end()), (6, 7));
    }

    /// A list nested more than `MAX_DEPTH` levels below its `<` is none, however its nesting is
    /// made: the type below `MAX_DEPTH - 1` brackets or lists is the deepest one allowed.
    #[test]
    fn type_argument_lists_should_end_at_max_depth() {
        for allowed in [true, false] {
            let n = MAX_DEPTH - 1 + usize::from(!allowed);
            let brackets = format!("f<{}i64{}>(x)", "[".repeat(n), "]".repeat(n));
            let lists = format!("f<{}i64{}>(x)", "A<".repeat(n), ">".repeat(n));
            assert_eq!(follows(&brackets), allowed, "{n} brackets");
            assert_eq!(follows(&lists), allowed, "{n} lists");
        }
    }

    /// A list that scans as one can still be too deep to parse, below the depth of its call.
    #[test]
    fn type_arguments_should_count_toward_the_nesting_limit() {
        let source = |name: &str, types: &str| {
            let n = MAX_DEPTH - 8;
            format!(
                "fn main() {{ {}{name}<{types}>(x){} }}",
                "(".repeat(n),
                ")".repeat(n)
            )
        };
        let nested = format!("{}i64{}", "[".repeat(9), "]".repeat(9));
        // The parser recurses as deep as the limit allows, which the compiler's own stack holds.
        let codes = crate::on_compiler_stack(|| {
            ["f", "s.f"].map(|name| {
                let fits = parsed(&source(name, "i64")).diagnostics.len();
                (fits, parse_err(&source(name, &nested)).0)
            })
        });
        // One `i64` fits where nine brackets around it do not.
        assert_eq!(codes, [(0, Code::NestingTooDeep); 2]);
    }

    /// The type arguments of a pattern can be too deep to parse though they scan as a list, and
    /// that is the error, not E0123.
    #[test]
    fn pattern_type_arguments_should_count_toward_the_nesting_limit() {
        let source = |types: &str| {
            let n = MAX_DEPTH - 8;
            format!(
                "fn main() {{ {}match e {{ A<{types}>.B(x) => 1 }}{} }}",
                "(".repeat(n),
                ")".repeat(n)
            )
        };
        let nested = format!("{}i64{}", "[".repeat(9), "]".repeat(9));
        // The parser recurses as deep as the limit allows, which the compiler's own stack holds.
        let codes = crate::on_compiler_stack(|| {
            [source("i64"), source(&nested)].map(|src| parse_err(&src).0)
        });
        // One `i64` fits where nine brackets around it do not.
        assert_eq!(codes, [Code::PatternForm, Code::NestingTooDeep]);
    }

    /// Type arguments and value arguments are siblings in the tree, so the deepest of them counts,
    /// whichever way the call is written, and what follows the call sits above both.
    #[test]
    fn type_arguments_and_value_arguments_should_be_siblings() {
        fn brackets(n: usize) -> String {
            format!("{}i64{}", "[".repeat(n), "]".repeat(n))
        }
        let access = ".x".repeat(30);
        let cases = [
            // Two lists of 130 brackets side by side are 134 deep, not 264.
            format!("f<{0}>(g<{0}>())", brackets(130)),
            format!("s.f<{0}>(g<{0}>())", brackets(130)),
            // 240 brackets with 30 accesses above them are too deep either way.
            format!("f<{}>(){access}", brackets(240)),
            format!("s.f<{}>(){access}", brackets(240)),
        ];
        // The most brackets that a call of `callee` can hold in its type argument.
        let most = |callee: &str| {
            (0..MAX_DEPTH)
                .take_while(|&n| {
                    let src = format!("fn main() {{ {callee}<{}>() }}", brackets(n));
                    parsed(&src).diagnostics.is_empty()
                })
                .last()
                .expect("a call without brackets should parse")
        };
        // The parser recurses as deep as the limit allows, which the compiler's own stack holds.
        let (found, free, method) = crate::on_compiler_stack(|| {
            let found = cases.map(|src| {
                parsed(&format!("fn main() {{ {src} }}"))
                    .diagnostics
                    .first()
                    .map(Diagnostic::code)
            });
            (found, most("f"), most("s.f"))
        });
        let too_deep = Some(Code::NestingTooDeep);
        assert_eq!(found, [None, None, too_deep, too_deep]);
        // The receiver is a level of its own, so the method call's arguments sit one lower.
        assert_eq!(free, method + 1);
    }

    /// What `src`, a statement, makes the nesting report name: the form of the height at which
    /// the parser stopped.
    fn nesting_form(src: &str) -> &'static str {
        crate::on_compiler_stack(|| {
            parser_at(src, 0, |p| {
                let error = p.stmt().expect_err("the statement should be too deep");
                assert_eq!(error.code(), Code::NestingTooDeep, "{src}");
                match p.depth.form() {
                    NestingForm::Tree => "tree",
                    NestingForm::Chain => "chain",
                    NestingForm::ElseIf => "else if",
                    NestingForm::Alias(_) => unreachable!("only the checker makes an alias"),
                }
            })
        })
    }

    /// The span of the `NestingTooDeep` error of the statement `src`, as byte offsets.
    fn nesting_span(src: &str) -> (usize, usize) {
        crate::on_compiler_stack(|| {
            parser_at(src, 0, |p| {
                let error = p.stmt().expect_err("the statement should be too deep");
                assert_eq!(error.code(), Code::NestingTooDeep, "{src}");
                (error.span().start(), error.span().end())
            })
        })
    }

    /// An `else if` chain reports at the `else if` of the last link on the path, whatever token
    /// in its branch passed the limit.
    #[test]
    fn nesting_report_should_point_at_the_last_else_if() {
        let head = "if a { }";
        let link = " else if a { }";
        let src = format!("{head}{}", link.repeat(300));
        // Two levels sit above the chain, so the `else if` with index `MAX_DEPTH - 2` is the first
        // whose `if` passes the limit. The caret is on its `else if`, not on the `a` that follows.
        let at = head.len() + link.len() * (MAX_DEPTH - 2) + 1;
        assert_eq!(nesting_span(&src), (at, at + "else if".len()));
    }

    /// An expression that is deep enough to pass the limit under a run of 150 `else if`s.
    fn deep_expr() -> String {
        format!("print({}1{})", "(".repeat(100), ")".repeat(100))
    }

    /// The report points at the last `else if` of the longest run of `else if`s on the deepest
    /// path, not at a shorter run, whether the shorter run is before the longest run, after it or
    /// inside a block under it.
    #[test]
    fn nesting_report_should_point_at_the_longest_else_if_run() {
        let head = "if a { }";
        let link = " else if a { }";
        let deep = deep_expr();
        let else_if = |src: &str, at: usize| {
            assert_eq!(&src[at..][.."else if".len()], "else if", "{src}");
            (at, at + "else if".len())
        };
        // A run of 200, then a block with a chain of one `else if` that passes the limit.
        let src = format!(
            "{head}{} else if a {{ if a {{ }} else if a {{ {deep} }} }}",
            link.repeat(199)
        );
        let at = head.len() + link.len() * 199 + 1;
        assert_eq!(nesting_span(&src), else_if(&src, at), "run of 200");
        // A run of 150, a block, and a run that is shorter when the limit passes.
        let block = " else { if a { }";
        let src = format!("{head}{}{block}{} }}", link.repeat(150), link.repeat(150));
        let at = head.len() + link.len() * 149 + 1;
        assert_eq!(nesting_span(&src), else_if(&src, at), "first run longer");
        // A run of 20, a block, a run of 150, then a chain of one `else if` that passes the limit.
        // The longest run is not the first run on the path.
        let src = format!(
            "{head}{}{block}{} else if a {{ if a {{ }} else if a {{ {deep} }} }} }}",
            link.repeat(20),
            link.repeat(149)
        );
        let at = head.len() + link.len() * 20 + block.len() + link.len() * 149 + 1;
        assert_eq!(nesting_span(&src), else_if(&src, at), "later run longer");
    }

    /// Of two runs of the same length on one path, the report keeps the later one, so it stays
    /// near the place where the limit passed. Many short runs of the same length give the same.
    /// This holds at the base too: it guards the tie rule.
    #[test]
    fn nesting_report_should_keep_the_later_run_on_a_tie() {
        let head = "if a { }";
        let link = " else if a { }";
        let block = " else { if a { }";
        let deep = deep_expr();
        let src = format!(
            "{head}{}{block}{} else if a {{ {deep} }} }}",
            link.repeat(100),
            link.repeat(99)
        );
        let at = src.rfind("else if").expect("the last `else if`");
        assert_eq!(nesting_span(&src), (at, at + "else if".len()), "two runs");
        // 90 units of a run of 3: the limit passes after about 50 units, and the report is in
        // that unit, not in the first one.
        let unit = format!("{}{block}", link.repeat(3));
        let src = format!("{head}{}{}", unit.repeat(90), " }".repeat(90));
        let (start, end) = nesting_span(&src);
        assert_eq!(&src[start..end], "else if", "{src}");
        assert!(start > head.len() + unit.len() * 40, "reported at {start}");
    }

    /// A run in a block beside the deepest path never draws the report, even when it is longer.
    /// This holds at the base too: it guards the scope of the longest run.
    #[test]
    fn nesting_report_should_ignore_a_run_beside_the_path() {
        let head = "if a { }";
        let link = " else if a { }";
        let deep = deep_expr();
        // The then-block of the outer `if` holds a run of 200. The path is the run of 160 after it.
        let src = format!(
            "if a {{ {head}{} }}{} else if a {{ {deep} }}",
            link.repeat(200),
            link.repeat(159)
        );
        let at = src.rfind("else if").expect("the last `else if`");
        assert_eq!(nesting_span(&src), (at, at + "else if".len()));
    }

    /// When `if` is on the line after `else`, the report underlines `else` alone. When `else` and
    /// `if` are on one line, it underlines both, however much space is between them.
    #[test]
    fn nesting_report_should_underline_only_else_across_a_line_break() {
        let head = "if a { }";
        let else_len = "else".len();
        // One run, `if` on the next line, with `\n` and with `\r\n`.
        for line_end in ["\n", "\r\n"] {
            let link = format!(" else{line_end}if a {{ }}");
            let src = format!("{head}{}", link.repeat(300));
            let at = head.len() + link.len() * (MAX_DEPTH - 2) + 1;
            assert_eq!(&src[at..][..else_len], "else", "{src:?}");
            assert_eq!(nesting_span(&src), (at, at + else_len), "{line_end:?}");
        }
        // A run of 150 with a line break in each link, then a block and a shorter run.
        let link = " else\nif a { }";
        let src = format!(
            "{head}{}{}{} }}",
            link.repeat(150),
            " else {\nif a { }",
            link.repeat(150)
        );
        let at = head.len() + link.len() * 149 + 1;
        assert_eq!(&src[at..][..else_len], "else", "{src}");
        assert_eq!(nesting_span(&src), (at, at + else_len), "first run longer");
        // Spaces on one line are no line break: the span stays whole.
        let link = " else    if a { }";
        let src = format!("{head}{}", link.repeat(300));
        let at = head.len() + link.len() * (MAX_DEPTH - 2) + 1;
        assert_eq!(
            nesting_span(&src),
            (at, at + "else    if".len()),
            "one line"
        );
        // A line break right after `if` is outside the span: the span stays whole.
        let link = " else if\na { }";
        let src = format!("{head}{}", link.repeat(300));
        let at = head.len() + link.len() * (MAX_DEPTH - 2) + 1;
        assert_eq!(
            nesting_span(&src),
            (at, at + "else if".len()),
            "break after `if`"
        );
    }

    /// A chain of operators reports at the token that passed the limit, even under an `else if`.
    #[test]
    fn nesting_report_of_a_chain_should_stay_on_the_token_that_passed_the_limit() {
        // A chain keeps the report on the token where the limit passed, an operator.
        let chain = vec!["a"; 300].join(" + ");
        let (start, end) = nesting_span(&chain);
        assert_eq!(&chain[start..end], "+");
        assert_eq!(
            start,
            4 * (MAX_DEPTH - 1) + 2,
            "the `+` that passed the limit"
        );
        // An `else if` high on the path does not draw the report when a chain made it deep.
        let mixed = format!("if a {{ }} else if a {{ {chain} }}");
        let (start, end) = nesting_span(&mixed);
        assert_eq!(&mixed[start..end], "+");
    }

    /// The links of one chain add across the operators, postfix steps and prefix operators on
    /// the deepest path; `else if`s and nested brackets have forms of their own. The fixtures
    /// `E0071-chain-mixed`, `E0071-chain-prefix`, `E0071-else-if` and `E0071-tree` pin the text.
    #[test]
    fn nesting_report_should_name_what_made_the_deepest_path_deep() {
        let else_ifs = format!("if a {{ }}{}", " else if a { }".repeat(300));
        for (what, src, form) in [
            ("one operator", vec!["1"; 300].join(" + "), "chain"),
            // The links are in two loops: `*` is in the right side of the first `+`.
            (
                "two operators",
                format!("1 + {}{}", vec!["1"; 130].join(" * "), " + 1".repeat(130)),
                "chain",
            ),
            // Postfix steps, then `+`.
            (
                "postfix then operator",
                format!("a{}{}", ".b()".repeat(150), " + 1".repeat(110)),
                "chain",
            ),
            // `??` and `&&`.
            (
                "coalesce then operator",
                format!("a{}{}", " ?? a".repeat(200), " && a".repeat(60)),
                "chain",
            ),
            ("prefix operators", format!("{}a", "!".repeat(300)), "chain"),
            ("negations", format!("{}a", "- ".repeat(300)), "chain"),
            ("else if chain", else_ifs.clone(), "else if"),
            (
                "else if chain as a value",
                format!("let y = {else_ifs}"),
                "else if",
            ),
            (
                "nested parentheses",
                format!("{}1{}", "(".repeat(300), ")".repeat(300)),
                "tree",
            ),
        ] {
            assert_eq!(nesting_form(&src), form, "{what}");
        }
    }

    /// Every operand opens a nested scan, so the cutoff at `MAX_DEPTH` is what keeps a chain
    /// linear: each token is then scanned by at most `MAX_DEPTH + 1` of them.
    #[test]
    fn long_comparison_chains_should_scan_linearly() {
        let scanned = |n: usize, unit: &str| {
            let chain = vec![unit; n].join(" < ");
            SCANNED.set(0);
            first_fn(&format!("fn main() {{ print({chain}) }}"));
            SCANNED.get()
        };
        // A parenthesis after each `<` opens a type to scan, and `any` is a name there.
        for unit in ["a", "a < (a)", "a < any"] {
            crate::mir::tests::assert_near_linear(
                scanned(1000, unit),
                scanned(2000, unit),
                scanned(3000, unit),
            );
        }
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

    /// Each form of `let ... else` reads the pattern as `if let` does.
    #[test]
    fn let_else_should_parse_each_form() {
        let rows = [
            (
                "let v = o else { return }",
                "(let-else v o (block (return)))",
            ),
            (
                "let _ = o else { return }",
                "(let-else _ o (block (return)))",
            ),
            (
                "let mut n = o else { return }",
                "(let-else mut n o (block (return)))",
            ),
            (
                "let Circle(r) = s else { return 0.0 }",
                "(let-else (Circle r) s (block (return 0.0)))",
            ),
            (
                "let Circle(r) | Square(r) = s else { return 0.0 }",
                "(let-else (| (Circle r) (Square r)) s (block (return 0.0)))",
            ),
            (
                "let None = o else { return }",
                "(let-else None o (block (return)))",
            ),
            (
                "let Shape.Dot = s else { return 0 }",
                "(let-else Shape.Dot s (block (return 0)))",
            ),
            (
                "let Option.None = o else { return }",
                "(let-else Option.None o (block (return)))",
            ),
            (
                "let Shape.Dot | Shape.Line = s else { return 0 }",
                "(let-else (| Shape.Dot Shape.Line) s (block (return 0)))",
            ),
        ];
        for (src, expected) in rows {
            assert_eq!(expr(src), expected, "{src}");
        }
    }

    /// The `}` rule looks at the last token of the value, and only there: `)` and `]` end values
    /// that hold a block.
    #[test]
    fn let_else_brace_rule_should_look_only_at_the_last_token() {
        for src in [
            "let v = f(if c { a } else { b }) else { return }",
            "let v = xs[{ 0 }] else { return }",
        ] {
            assert!(expr(src).starts_with("(let-else v "), "{src}");
        }
        for (src, col) in [
            ("let v = o ?? { p } else { return }", 21),
            ("let Some(v) = match k { _ => o } else { return }", 27),
            ("let v = { o } else { return }", 21),
            ("let v = loop { break o } else { return }", 21),
        ] {
            assert_eq!(
                expr_err(src),
                (Code::LetElseValueEndsInBrace, 1, col),
                "{src}"
            );
        }
        let d = parse_diagnostic("fn main() {\n    let v = o ?? { p } else { return }\n}");
        assert_eq!(
            fix_edits(&d).map(|(safe, edits)| (safe, edits.len())),
            Some((true, 2))
        );
        assert_eq!(
            fixed("fn main() {\n    let v = o ?? { p } else { return }\n}"),
            "fn main() {\n    let v = (o ?? { p }) else { return }\n}"
        );
    }

    /// A jump with no value stops before `else`, so it is the value's last operand.
    #[test]
    fn let_else_should_follow_a_jump_with_no_value() {
        assert_eq!(
            expr("let v = o ?? return else { return }"),
            "(let-else v (?? o (return)) (block (return)))"
        );
        assert_eq!(
            expr("let v = o ?? break else { break }"),
            "(let-else v (?? o (break)) (block (break)))"
        );
    }

    /// A `let` with no `else` and a name parses as it did before let-else; the errors of the other
    /// forms keep the base text, code and position.
    #[test]
    fn let_without_else_should_parse_as_today() {
        for src in [
            "let x = 1",
            "let x: i64 = 1",
            "let _ = f()",
            "let mut x = 1",
            "let x = if c { 1 } else { return 0 }",
        ] {
            assert!(expr(src).starts_with("(let "), "{src}");
        }
        for (src, code, col) in [
            ("let = 1", Code::Expected, 17),
            ("let loop = 1", Code::Expected, 17),
            ("let p.x = 5", Code::Expected, 18),
            ("let mut p.x = 5", Code::Expected, 22),
            ("let Shape.Dot = s", Code::Expected, 22),
            // A value that fails to parse is no reason to hide the `.`: no `else` can follow it.
            ("let Shape.Dot = 1 +", Code::Expected, 22),
            ("let p.x = 1 + * 2", Code::Expected, 18),
            ("let Shape.Dot =", Code::Expected, 22),
            ("let Option.None = 1 +", Code::Expected, 23),
            (
                "let Shape.Dot = if c { 1 } else { 2 } +",
                Code::Expected,
                22,
            ),
            ("let 99999999999999999999 = 1", Code::IntegerTooLarge, 17),
        ] {
            assert_eq!(expr_err(src), (code, 1, col), "{src}");
        }
        // The `else` that follows makes it a `let ... else` with a bad value: the value's error.
        for (src, col) in [
            ("let Shape.Dot = mk(1 +) else { return }", 35),
            ("let Shape.Dot = s + else { return }", 33),
            ("let Option.None = o. else { return }", 34),
            (
                "let Shape.Dot = if c { 1 + } else { 2 } else { return }",
                40,
            ),
            ("let Shape.Dot = f({ 1 + }) else { return }", 37),
        ] {
            assert_eq!(expr_err(src), (Code::Expected, 1, col), "{src}");
        }
        // An `else` on the next line, or in another statement, is no part of the `let`.
        for src in [
            "fn main() {\n    let Shape.Dot = (1 +)\n    else { return }\n}",
            "fn main() {\n    let Shape.Dot = (1 +); if c { 1 } else { 2 }\n}",
        ] {
            assert_eq!(parse_err(src), (Code::Expected, 2, 14), "{src}");
        }
    }

    /// GIVEN a `let` with a type, and an `else` after its value, in a name, `mut`, `_`,
    /// pattern or qualified form, on the same line or on the next
    /// WHEN the parser reads it
    /// THEN the error is E0011 at the `: type`, with one fix that removes it
    /// AND the fix is unsafe: the parser stops at the type, so it has not read the rest of the
    /// statement, and the fixed source can have an error of its own
    /// AND the fix also joins an `else` on a later line to the value, when an edit can
    /// AND the fixed source has the errors in `after`
    /// AND a `let` with a type and no `else`, and a `const`, report nothing
    #[test]
    fn let_else_with_a_type_should_be_reported_at_the_type() {
        // The columns: the `let`, the text of the type with its colon, and the codes that the
        // fixed source reports.
        let rows = [
            ("let w: i64 = o else { return }", ": i64", vec![]),
            ("let mut w: i64 = o else { return }", ": i64", vec![]),
            ("let _: i64 = o else { return }", ": i64", vec![]),
            ("let Some(w): i64? = o else { return }", ": i64?", vec![]),
            ("let Shape.Sq: Shape = e else { return }", ": Shape", vec![]),
            // The fix joins the `else` to the value, so that the fixed source has no E0006: two
            // edits. The gap can hold a comment or a blank line.
            ("let w: i64 = o\n    else { return }", ": i64", vec![]),
            (
                "let Some(w): i64? = o\n    else { return }",
                ": i64?",
                vec![],
            ),
            (
                "let w: i64 = o\n    // why\n    else { return }",
                ": i64",
                vec![],
            ),
            ("let w: i64 = o\n\n    else { return }", ": i64", vec![]),
            // No edit can repair these: the fixed source has the `else` on its own line, which is
            // E0006, and the fix has one edit.
            (
                "let x: i64 = { 1 }\n    else { return }",
                ": i64",
                vec![Code::ElseOnNewLine],
            ),
            (
                "let w: i64 = o\n    else return",
                ": i64",
                vec![Code::ElseOnNewLine],
            ),
            // Controls: an `else` on the line already, and no `=`, give one edit.
            (
                "let Some(w): i64? = o else return\n    else { x }",
                ": i64?",
                vec![Code::Expected],
            ),
            (
                "let Some(w): i64?\n    else { return }",
                ": i64?",
                vec![Code::Expected],
            ),
            // The span of the type covers a `( )`, and a `>` that a `>=` closes.
            ("let w: (i64) = o else { return }", ": (i64)", vec![]),
            ("let w: (i64) = o\n    else { return }", ": (i64)", vec![]),
            ("let w: Box<i64>= o else { return }", ": Box<i64>", vec![]),
            ("let Some(w): (i64) = o else { return }", ": (i64)", vec![]),
            (
                "let Some(w): Box<i64>= o else { return }",
                ": Box<i64>",
                vec![],
            ),
            // A pattern with no `else` reports the type first. Then the checker, not the parser,
            // reports the pattern (E0174).
            ("let Some(w): i64? = o", ": i64?", vec![]),
            // The type comes first: the `mut` is reported once the type is gone.
            (
                "let mut _: i64 = o else { return }",
                ": i64",
                vec![Code::MutOnUnderscore],
            ),
            // The rest of the statement has an error that the parser has not read yet.
            (
                "let x: i64 = { 1 } else { return }",
                ": i64",
                vec![Code::LetElseValueEndsInBrace],
            ),
            (
                "let x: i64 = if c { 1 } else { 2 } else { return }",
                ": i64",
                vec![Code::LetElseValueEndsInBrace],
            ),
            (
                "let Some(w): i64 = { 1 } else { return }",
                ": i64",
                vec![Code::LetElseValueEndsInBrace],
            ),
            ("let x: i64 = o else", ": i64", vec![Code::Expected]),
            ("let x: i64 = o else return", ": i64", vec![Code::Expected]),
            (
                "let x: i64 = o else { return } x",
                ": i64",
                vec![Code::Expected],
            ),
        ];
        for (line, ty, after) in rows {
            let src = format!("fn main() {{\n    {line}\n}}");
            let d = parse_diagnostic(&src);
            let at = d.span();
            assert_eq!(&src[at.start()..at.end()], ty, "{src}");
            assert_eq!(d.fixes().len(), 1, "{src}");
            let (safe, edits) = fix_edits(&d).expect("the error has a fix");
            assert!(!safe, "{src}");
            // A fix that joins an `else` on a later line has two edits. Each row with a line
            // break and no error after the fix is such a row.
            let joins = line.contains('\n') && after.is_empty();
            assert_eq!(edits.len(), if joins { 2 } else { 1 }, "{src}");
            let fixed = apply_fix(&src, &d);
            let found: Vec<_> = parsed(&fixed)
                .diagnostics
                .iter()
                .map(Diagnostic::code)
                .collect();
            assert_eq!(found, after, "{fixed}");
        }
        for src in [
            "fn main() {\n    let w: i64 = o\n}",
            "const C: i64 = 1\nfn main() {}",
        ] {
            assert!(parsed(src).diagnostics.is_empty(), "{src}");
        }
    }

    /// GIVEN a qualified `let`, as `let a.b`, with a type and no `else` on its line or the next
    /// WHEN the parser reads it
    /// THEN the first mistake is the `.`: E0011 "expected `=`, found `.`", as for a `let` with a
    /// name and no type
    /// AND with an `else` on the line, the error is the type; with one on the next line, it is
    /// the type too, and its fix joins the `else`
    #[test]
    fn qualified_let_with_a_type_and_no_else_should_report_the_dot() {
        for (line, col) in [
            ("let a.b: i64 = 5", 10),
            ("let a.b: = 5", 10),
            ("let a.b: i64", 10),
            ("let Shape.Sq: Shape = e", 14),
            ("let mut a.b: i64 = 5", 14),
        ] {
            let src = format!("fn main() {{\n    {line}\n}}");
            assert_eq!(parse_err(&src), (Code::Expected, 2, col), "{src}");
        }
        for (line, after) in [
            ("let Shape.Sq: Shape = e else { return }", vec![]),
            ("let Shape.Sq: Shape = e\n    else { return }", vec![]),
        ] {
            let src = format!("fn main() {{\n    {line}\n}}");
            let d = parse_diagnostic(&src);
            assert_eq!(&src[d.span().start()..d.span().end()], ": Shape", "{src}");
            let fixed = apply_fix(&src, &d);
            let found: Vec<_> = parsed(&fixed)
                .diagnostics
                .iter()
                .map(Diagnostic::code)
                .collect();
            assert_eq!(found, after, "{fixed}");
        }
        let src = "fn main() {\n    let Shape.Sq: Shape = e\n    else { return }\n}";
        assert_eq!(
            apply_fix(src, &parse_diagnostic(src)),
            "fn main() {\n    let Shape.Sq = e else { return }\n}"
        );
    }

    /// GIVEN a typed `let ... else` with CRLF line endings and the `else` on the next line
    /// WHEN the fix is applied
    /// THEN the fixed source parses with no error
    /// AND it holds no `\r` that a `\n` does not follow
    #[test]
    fn a_let_type_fix_should_join_across_crlf() {
        let src = "fn main() {\r\n    let w: i64 = o\r\n    else { return }\r\n}";
        let fixed = apply_fix(src, &parse_diagnostic(src));
        assert_eq!(fixed, "fn main() {\r\n    let w = o else { return }\r\n}");
        assert!(parsed(&fixed).diagnostics.is_empty(), "{fixed:?}");
        assert!(!fixed.replace("\r\n", "").contains('\r'), "{fixed:?}");
    }

    /// A pattern that is not a name and has no `else` is no parse error: the parser keeps it as
    /// a `let ... else` with no block, and the checker reports it by the value's type.
    #[test]
    fn let_pattern_without_else_should_be_kept_with_no_report() {
        let src = "fn main() {\n    let Some(v) = o\n    print(v)\n}";
        let parsed = parsed(src);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        assert!(parsed.checkable);
        assert_eq!(parsed.program.broken, []);
        let [Item::Function(main)] = parsed.program.items.as_slice() else {
            panic!("expected `main` alone, found {:?}", parsed.program.items);
        };
        assert!(
            matches!(
                main.body.stmts.first().map(|s| &s.kind),
                Some(StmtKind::LetElse {
                    else_block: None,
                    ..
                })
            ),
            "the first statement should be a `let ... else` with no block"
        );
    }

    /// `let mut` before a pattern that is not a name is E0011 at the `mut`; a qualified variant
    /// with no payload keeps the error of `let mut p.x`.
    #[test]
    fn let_mut_before_a_pattern_should_be_an_error_at_mut() {
        for src in [
            "let mut Some(v) = o else { return }",
            "let mut None = o else { return }",
            "let mut 1 = o else { return }",
            "let mut A(v) | B(v) = o else { return }",
        ] {
            assert_eq!(expr_err(src), (Code::Expected, 1, 17), "{src}");
        }
        assert_eq!(
            expr_err("let mut Shape.Dot = s else { return }"),
            (Code::Expected, 1, 26)
        );
        assert_eq!(
            expr_err("let mut _ = o else { return }"),
            (Code::MutOnUnderscore, 1, 21)
        );
    }

    /// GIVEN a `let` whose pattern is written with braces, as `Rect { w, h }`, with labels, with
    /// a qualified name, or with a lower-case name, beside the `if let` of the same pattern
    /// WHEN the parser reads each
    /// THEN the `let` gives the same diagnostic as the `if let`: code, span and fix edits (the
    /// fixture E0123-braces-let pins the text)
    /// AND `let mut` before such a pattern is E0011 at the `mut`
    #[test]
    fn braced_let_pattern_should_be_reported_as_the_if_let_pattern() {
        for pattern in [
            "Rect { w, h }",
            "Rect { w: a, h: b }",
            "Shape.Re { w, h }",
            "Shape.None { }",
            "x { }",
        ] {
            // `    let ` and ` if let ` are the same width, so the spans agree too.
            let with_let = format!("fn main() {{\n    let {pattern} = s else {{ return }}\n}}");
            let with_if_let = format!("fn main() {{\n if let {pattern} = s {{ }}\n}}");
            let (a, b) = (parse_diagnostic(&with_let), parse_diagnostic(&with_if_let));
            assert_eq!(a.code(), Code::PatternForm, "{with_let}");
            let shape = |d: &Diagnostic| {
                let fixes: Vec<_> = d
                    .fixes()
                    .iter()
                    .map(|f| {
                        let edits: Vec<_> = f
                            .edits()
                            .iter()
                            .map(|e| (e.span(), e.replacement().to_owned()))
                            .collect();
                        (f.safe(), edits)
                    })
                    .collect();
                (d.code(), d.span(), fixes)
            };
            assert_eq!(shape(&a), shape(&b), "{pattern}");
        }
        assert_eq!(
            expr_err("let mut Rect { w, h } = s else { return }"),
            (Code::Expected, 1, 17)
        );
    }

    /// GIVEN a name followed by `{ ... }` in each place where a pattern stands
    /// WHEN the parser reads it
    /// THEN it reports the braced pattern (E0123) only when a pattern of that place can end at the
    /// `}` that closes the `{`: in a `let`, `if let` or `while let`, `=`, `:` or `|`; in a match
    /// arm, `=>`, `->`, `if` or `|`, or a line break and then `=>` or `|`; in a sub-pattern, `,`,
    /// `)`, `}` or `|`, or a line break and then `,`, `}` or `|`
    /// AND otherwise it reports the `{` as a stray token (E0011) with no fix, as `let a { }` does,
    /// before it reads anything inside the braces
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one row per place and token, so the table reads in one place"
    )]
    fn a_braced_name_should_be_a_pattern_only_when_a_pattern_can_end_at_its_brace() {
        // (statement, code, line, column of the error)
        let rows: Vec<(String, Code, usize, usize)> =
            vec![
            // A stray `{`: the pattern cannot end at the `}`.
            ("    let a { }".into(), Code::Expected, 2, 11),
            ("    let a { x }".into(), Code::Expected, 2, 11),
            ("    let a {\n    }".into(), Code::Expected, 2, 11),
            ("    let a { w: 1 }".into(), Code::Expected, 2, 11),
            ("    let Shape.Re { }".into(), Code::Expected, 2, 18),
            ("    if let a { } {\n    }".into(), Code::Expected, 2, 14),
            ("    while let a { } {\n    }".into(), Code::Expected, 2, 17),
            ("    if let a { print(a) }".into(), Code::Expected, 2, 14),
            ("    match e { E.P { x } }".into(), Code::Expected, 2, 19),
            // The `{` is the error, and the body after it is not read as patterns.
            ("    if let a {\n        print(a)\n    }".into(), Code::Expected, 2, 14),
            ("    while let a {\n        print(a)\n    }".into(), Code::Expected, 2, 17),
            ("    let a {\n        print(a)\n    }".into(), Code::Expected, 2, 11),
            ("    if let E.P {\n        print(1)\n    }".into(), Code::Expected, 2, 16),
            (
                "    if let a {\n        let b = 1\n        print(b)\n    } else {\n        print(2)\n    }"
                    .into(),
                Code::Expected,
                2,
                14,
            ),
            (
                "    if let a {\n        if let b {\n            print(b)\n        }\n    }".into(),
                Code::Expected,
                2,
                14,
            ),
            ("    if let a { x + 1 }".into(), Code::Expected, 2, 14),
            ("    if let a { let b = 1 }".into(), Code::Expected, 2, 14),
            ("    if let a { return }".into(), Code::Expected, 2, 14),
            ("    while let a { x + 1 }".into(), Code::Expected, 2, 17),
            (in_arm("E.P { x }"), Code::Expected, 3, 13),
            // A `;` is no line break: it ends the arm before the `=>`.
            (in_arm("E.P { x }; => print(1)"), Code::Expected, 3, 13),
            // A line break and then another token does not end a sub-pattern.
            (
                in_arm("E.P {\n            x: R { w }\n            y\n        } => print(1)"),
                Code::Expected,
                4,
                18,
            ),
            // A block in an arm is no sub-pattern: it does not keep the state of the pattern.
            (
                "    match e {\n        A(x) => {\n            let a { }\n        }\n    }".into(),
                Code::Expected,
                4,
                19,
            ),
            // After a sub-pattern in a sub-pattern, the outer one is still a sub-pattern.
            (in_arm("Some(S(x) | R { w }) => 1"), Code::PatternForm, 3, 23),
            // After a sub-pattern, the arm is the place again: `|` and then the braced name.
            (in_arm("E.P(x, y) | a { } => 1"), Code::PatternForm, 3, 23),
            // An `if` after a line break starts a statement, not a guard.
            ("    let a { }\n    if true { }".into(), Code::Expected, 2, 11),
            // A token ends a pattern only in its own place. `=>`, `->` and `if` end the pattern
            // of an arm. `=` and `:` end the pattern of a `let`. The place of an outer pattern
            // does not count in a sub-pattern or in an interpolation.
            ("    if let a { } if true { }".into(), Code::Expected, 2, 14),
            ("    if let a { } -> 1".into(), Code::Expected, 2, 14),
            ("    if let a { } => 1".into(), Code::Expected, 2, 14),
            ("    while let a { } if true { }".into(), Code::Expected, 2, 17),
            ("    let a { } if true".into(), Code::Expected, 2, 11),
            ("    let a { } -> 1".into(), Code::Expected, 2, 11),
            ("    if let a { }\n    => 1".into(), Code::Expected, 2, 14),
            (in_arm("a { } = 1 => 2"), Code::Expected, 3, 11),
            (in_arm("a { } : i64 => 2"), Code::Expected, 3, 11),
            (in_arm("E.P(a { } if true) => 2"), Code::Expected, 3, 15),
            (
                in_arm(r#"E.P { "a{ f(if let b { }) }" } => print(1)"#),
                Code::Expected,
                3,
                30,
            ),
            // A guard on the next line: the `{` is the mistake.
            (in_arm("E.P { x }\n        if x > 0 => print(x)"), Code::Expected, 3, 13),
            // A pattern ends at the `}`: one row for each token of the rule.
            ("    let a { } = s".into(), Code::PatternForm, 2, 11),
            ("    let a { } = s else { return }".into(), Code::PatternForm, 2, 11),
            ("    let a {} = 1".into(), Code::PatternForm, 2, 11),
            ("    let R { w }: R = r".into(), Code::PatternForm, 2, 11),
            (in_arm("E.P { x } => print(x)"), Code::PatternForm, 3, 13),
            (in_arm("E.P { x } -> print(x)"), Code::PatternForm, 3, 13),
            (in_arm("E.P { x } if x > 0 => print(x)"), Code::PatternForm, 3, 13),
            (in_arm("E.P { x } | E.Q => print(1)"), Code::PatternForm, 3, 13),
            (in_arm("E.Q | E.P { x } => print(1)"), Code::PatternForm, 3, 19),
            (in_arm("Some(R { w }) => print(w)"), Code::PatternForm, 3, 16),
            (in_arm("Some(R { w }, 1) => print(w)"), Code::PatternForm, 3, 16),
            // The inner pattern is the first one that ends at its `}`.
            (in_arm("E.P { x: R { w } } => print(1)"), Code::PatternForm, 3, 20),
            // A real braced pattern on many lines. The inner pattern is the first one to report.
            (
                in_arm("E.P {\n            x: R { w }\n        } => print(1)"),
                Code::PatternForm,
                4,
                18,
            ),
            (
                in_arm("E.P {\n            x,\n            y\n        } => print(1)"),
                Code::PatternForm,
                3,
                13,
            ),
            (
                in_arm("E.P {\n            x: R {\n                w\n            }\n        } => print(1)"),
                Code::PatternForm,
                4,
                18,
            ),
            (
                in_arm("Some(R {\n            w\n        }) => print(1)"),
                Code::PatternForm,
                3,
                16,
            ),
            (
                "    match o {\n        Some(\n            R {\n                w\n            }\n        ) => print(1)\n    }"
                    .into(),
                Code::PatternForm,
                4,
                15,
            ),
            (
                in_arm("E.P {\n            x: R { w },\n            y\n        } => print(1)"),
                Code::PatternForm,
                4,
                18,
            ),
            (
                in_arm("E.P {\n            x: R { w }\n            , y\n        } => print(1)"),
                Code::PatternForm,
                4,
                18,
            ),
            (
                in_arm("E.P {\n            x\n            , y\n        } => print(1)"),
                Code::PatternForm,
                3,
                13,
            ),
            (
                in_arm("E.P {\n            x\n        } | E.Q => print(1)"),
                Code::PatternForm,
                3,
                13,
            ),
            (
                in_arm("E.P {\n            x\n        } if x > 0 => print(x)"),
                Code::PatternForm,
                3,
                13,
            ),
            (
                in_arm("E.P {\n            x | y\n        } => print(1)"),
                Code::PatternForm,
                3,
                13,
            ),
            (
                "    let E.P {\n        x: R { w }\n    } = e else { return }".into(),
                Code::PatternForm,
                3,
                14,
            ),
            (
                "    if let E.P {\n        x: R { w }\n    } = e {\n    }".into(),
                Code::PatternForm,
                3,
                14,
            ),
            (
                "    while let E.P {\n        x,\n        y\n    } = e {\n    }".into(),
                Code::PatternForm,
                2,
                19,
            ),
            (
                "    let R {\n        w\n    }: R = r".into(),
                Code::PatternForm,
                2,
                11,
            ),
            // A `|` ends the pattern of a `let`, and `)` or `,` ends a sub-pattern of a `let`.
            ("    if let a { } | b = e { }".into(), Code::PatternForm, 2, 14),
            ("    if let E.P(a { }, b) = e { }".into(), Code::PatternForm, 2, 18),
            // An arm in an interpolation is an arm again: `=>` ends its pattern.
            (
                in_arm(r#"E.P { "a{ match n { b { } => 1 } }" } => 1"#),
                Code::PatternForm,
                3,
                31,
            ),
            // A line break and then `=>` or `|`: neither can start a statement.
            (in_arm("E.P { x }\n        => print(x)"), Code::PatternForm, 3, 13),
            (in_arm("E.P { x }\n        | E.Q => print(1)"), Code::PatternForm, 3, 13),
            (
                "    match o {\n        Some(\n            R { w }\n        ) => print(w)\n    }"
                    .into(),
                Code::PatternForm,
                4,
                15,
            ),
        ];
        for (stmt, code, line, column) in rows {
            let src = format!("fn main() {{\n{stmt}\n}}");
            let d = parse_diagnostic(&src);
            let (got_line, got_column) = line_col(&src, d.span().start());
            assert_eq!(
                (d.code(), got_line, got_column),
                (code, line, column),
                "{src}"
            );
            if code == Code::Expected {
                assert_eq!(&src[d.span().range()], "{", "{src}");
                assert!(d.fixes().is_empty(), "{src}");
            }
        }
        // `let mut` is left as it is: E0011 at the `mut`.
        let src = "fn main() {\n    let mut a { }\n}";
        let d = parse_diagnostic(src);
        assert_eq!(
            (d.code(), line_col(src, d.span().start())),
            (Code::Expected, (2, 9))
        );
        assert_eq!(&src[d.span().range()], "mut");
    }

    /// `pattern` as the pattern of a `match` arm, as one statement of a `main`.
    fn in_arm(pattern: &str) -> String {
        format!("    match e {{\n        {pattern}\n    }}")
    }

    /// `stmt` as the body of a `main`.
    fn in_main(stmt: &str) -> String {
        format!("fn main() {{\n{stmt}\n}}")
    }

    /// GIVEN a name and then `{ ... }` at the end of a block, on one line with the block's `}`
    /// WHEN the parser reads it
    /// THEN it reports the `{` as a stray token (E0011) with no fix, because no pattern can end
    /// at the `}`: the `}` of the block follows
    #[test]
    fn a_braced_name_at_the_end_of_a_block_should_report_the_brace() {
        let rows = [
            ("fn main() { let a { } }", 19),
            ("fn main() { if let a { } }", 22),
            ("fn main() { while let a { } }", 25),
            ("fn main() { let a { print(a) } }", 19),
            ("fn main() { if let a { print(a) } }", 22),
            ("fn main() { while let a { print(a) } }", 25),
            ("fn main() { if let a { x, y } }", 22),
            ("fn main() { let E.P { x } }", 21),
            ("fn main() { match e { E.P { x } } }", 27),
            ("fn main() { match e { a { } } }", 25),
            ("fn main() { f(if let a { }) }", 24),
            ("fn main() { let a { }, 1 }", 19),
        ];
        for (src, column) in rows {
            let d = parse_diagnostic(src);
            assert_eq!(
                (d.code(), line_col(src, d.span().start())),
                (Code::Expected, (1, column)),
                "{src}"
            );
            assert_eq!(&src[d.span().range()], "{", "{src}");
            assert!(d.fixes().is_empty(), "{src}");
        }
    }

    /// GIVEN a braced pattern, where a pattern can end at its `}`, that has an error inside
    /// WHEN the parser reads it
    /// THEN the error inside is the error, as it was before the parser decided at the `{`
    /// AND a `;` between the sub-patterns is still an error at the `;`
    #[test]
    fn an_error_in_a_braced_pattern_should_stay_when_a_pattern_ends_at_its_brace() {
        let arm = |pattern: &str| in_main(&in_arm(pattern));
        let interpolated = Code::InterpolatedPattern;
        let rows = [
            (arm("E.P { x: 1 + 2 } => print(1)"), Code::Expected, 3, 20),
            (arm("E.P { x: y z } => print(1)"), Code::Expected, 3, 20),
            (
                in_main("    let E.P { x: 1 + 2 } = e else { return }"),
                Code::Expected,
                2,
                20,
            ),
            (arm("E.P { Foo::Bar } => print(1)"), Code::MatchIdiom, 3, 18),
            (
                arm("E.P { x: Option<i64>.Some(v) } => print(1)"),
                Code::PatternForm,
                3,
                24,
            ),
            (arm("E.P { R(w: a) } => print(1)"), Code::PatternForm, 3, 17),
            (arm("E.P { \"a{b}\" } => print(1)"), interpolated, 3, 17),
            (arm("E.P { x; y } => print(1)"), Code::Expected, 3, 16),
            (arm("E.P { x;, y } => print(1)"), Code::Expected, 3, 16),
            (
                arm("E.P { x: R { w }; } => print(1)"),
                Code::PatternForm,
                3,
                20,
            ),
        ];
        for (src, code, line, column) in rows {
            let d = first_syntax_diagnostic(&src);
            assert_eq!(
                (d.code(), line_col(&src, d.span().start())),
                (code, (line, column)),
                "{src}"
            );
        }
    }

    /// GIVEN 300 braces nested in a pattern, and 300 after `if let a` that has no `=`
    /// WHEN the parser reads them
    /// THEN the first reports that the nesting is too deep (E0071), because a pattern can end at
    /// its `}`
    /// AND the second reports the first `{` as a stray token (E0011), because no pattern can end
    /// there
    #[test]
    fn nested_braces_should_report_the_depth_only_in_a_pattern() {
        let deep = format!("{}x{} => print(1)", "a { ".repeat(300), " }".repeat(300));
        let pattern = in_main(&in_arm(&deep));
        let stmt = format!("    if let {}x{}", "a { ".repeat(300), " }".repeat(300));
        let stray = in_main(&stmt);
        // The parser recurses as deep as the limit allows, which the compiler's own stack holds.
        let (too_deep, stray) = crate::on_compiler_stack(|| {
            let d = parse_diagnostic(&stray);
            (
                parse_diagnostic(&pattern).code(),
                (d.code(), line_col(&stray, d.span().start())),
            )
        });
        assert_eq!(too_deep, Code::NestingTooDeep);
        assert_eq!(stray, (Code::Expected, (2, 14)));
    }

    /// GIVEN an error in a sub-pattern of one function, and a stray braced name in the next
    /// WHEN the parser reads both
    /// THEN the second function reports the `{` as a stray token (E0011), because it is not in a
    /// sub-pattern
    #[test]
    fn an_error_in_a_sub_pattern_should_leave_the_next_function_outside_it() {
        let src =
            "fn f() {\n    match e {\n        A(*) => 1\n    }\n}\nfn main() {\n    let a { }\n}";
        let ds = parsed(src).diagnostics;
        let [_, second] = ds.as_slice() else {
            panic!("{src}: expected two diagnostics, got {ds:?}");
        };
        assert_eq!(
            (second.code(), line_col(src, second.span().start())),
            (Code::Expected, (7, 11))
        );
    }

    /// The first diagnostic of the lexer and the parser for `src`.
    fn first_syntax_diagnostic(src: &str) -> Diagnostic {
        match lex(src) {
            Err(d) => d,
            Ok(lexed) => parse(lexed, false)
                .diagnostics
                .into_iter()
                .next()
                .expect("the source should have a diagnostic"),
        }
    }

    /// GIVEN a name and then `{ ... }` that holds a lexer error
    /// WHEN the parser reads it
    /// THEN the lexer error is the error, also when the parse error at the `{` is earlier
    #[test]
    fn a_lexer_error_in_braces_should_come_before_an_error_at_the_brace() {
        let rows: Vec<(String, Code, usize, usize)> = vec![
            (
                in_main(&in_arm("E.P { \"abc } => print(1)")),
                Code::LiteralNotClosed,
                3,
                15,
            ),
            (
                in_main(&in_arm("Some(E.P { \"abc }) => print(1)")),
                Code::LiteralNotClosed,
                3,
                20,
            ),
            (
                in_main(&in_arm("E.P { \"abc\n        } => print(1)")),
                Code::LiteralNotClosed,
                3,
                15,
            ),
            (
                in_main(&in_arm("E.P { 'a } => print(1)")),
                Code::LiteralNotClosed,
                3,
                15,
            ),
            (
                in_main("    let E.P { \"abc } = e"),
                Code::LiteralNotClosed,
                2,
                15,
            ),
            (
                "fn main() {\n    match e {\n        E.P { \"abc".into(),
                Code::LiteralNotClosed,
                3,
                15,
            ),
            (
                in_main(&in_arm("E.P { x) } => print(1)")),
                Code::UnexpectedClosingBracket,
                3,
                16,
            ),
            (
                in_main(&in_arm("E.P { x # } => print(1)")),
                Code::UnexpectedCharacter,
                3,
                17,
            ),
            (
                in_main(&in_arm("E.P { x: 1 + \"abc } => print(1)")),
                Code::LiteralNotClosed,
                3,
                22,
            ),
            (
                in_main("    if let a { print(\"abc) }"),
                Code::LiteralNotClosed,
                2,
                22,
            ),
        ];
        for (src, code, line, column) in rows {
            let d = first_syntax_diagnostic(&src);
            assert_eq!(
                (d.code(), line_col(&src, d.span().start())),
                (code, (line, column)),
                "{src}"
            );
        }
    }

    /// GIVEN a braced pattern with a comment in a gap: after the `{`, between two sub-patterns,
    /// after a label or before the `}`
    /// WHEN the parser reports it (E0123)
    /// THEN the diagnostic has no fix, because the fix would delete the comment
    #[test]
    fn a_comment_between_the_sub_patterns_of_braces_should_leave_no_fix() {
        let rows = [
            in_main(&in_arm("E.P { x, // c\n            y } => print(x)")),
            in_main(&in_arm("E.P { // c\n            x, y } => print(x)")),
            in_main(&in_arm("E.P { x, y // c\n        } => print(x)")),
            in_main(&in_arm("E.P { w: // c\n            x } => print(x)")),
            in_main(&in_arm(
                "E.P {\n            // c\n            x\n        } => print(x)",
            )),
            in_main(&in_arm(
                "E.P {\n            x\n            // c\n        } => print(x)",
            )),
            in_main(&in_arm(
                "E.P { x,\n            // c\n            y } => print(x)",
            )),
            in_main(&in_arm("E.P { // c\n        } => print(1)")),
            in_main("    let E.P { x, // c\n        y } = e else { return }"),
            in_main("    if let E.P { x, // c\n        y } = e {\n    }"),
        ];
        for src in rows {
            let d = parse_diagnostic(&src);
            assert_eq!(d.code(), Code::PatternForm, "{src}");
            assert!(d.fixes().is_empty(), "{src}");
        }
    }

    /// Applies the fix of the first diagnostic of `src` until none is left or the first has no
    /// fix: the source, and how many fixes were applied.
    fn fixed_until_done(src: &str) -> (String, usize) {
        let mut now = src.to_string();
        for applied in 0..8 {
            let found = parsed(&now).diagnostics;
            let Some(d) = found.first() else {
                return (now, applied);
            };
            if d.fixes().is_empty() {
                return (now, applied);
            }
            now = apply_fix(&now, d);
        }
        panic!("{src}: the fixes do not end");
    }

    /// GIVEN a braced pattern on many lines, with a pattern inside, with a comment after another
    /// pattern, or with a comment inside a sub-pattern
    /// WHEN each fix is applied until no diagnostic is left
    /// THEN the source parses and every comment stays
    #[test]
    fn a_fix_of_braces_should_parse_and_keep_each_other_comment() {
        let rows = [
            (
                in_main(&in_arm(
                    "E.P {\n            x,\n            y\n        } => print(x)",
                )),
                in_main(&in_arm("E.P(x, y) => print(x)")),
            ),
            (
                in_main(&in_arm(
                    "E.P {\n            x: R { w }\n        } => print(1)",
                )),
                in_main(&in_arm("E.P(R(w)) => print(1)")),
            ),
            (
                in_main(&in_arm(
                    "R { w: Some( // keep\n            a), h } => print(h)",
                )),
                in_main(&in_arm("R(Some( // keep\n            a), h) => print(h)")),
            ),
            (in_main("    let a { } = s"), in_main("    let a() = s")),
        ];
        for (src, want) in rows {
            let (got, _) = fixed_until_done(&src);
            assert_eq!(got, want, "{src}");
            assert!(parsed(&got).diagnostics.is_empty(), "{src}");
        }
        // The arm is split by a comment, which no fix joins: the one fix of the braces stays.
        let src = in_main(&in_arm("E.P { x } // c\n        => print(x)"));
        let (now, applied) = fixed_until_done(&src);
        assert_eq!(applied, 1, "{src}");
        assert!(now.contains("// c"), "{now}");
    }

    /// GIVEN a braced pattern whose sub-pattern is continued by a line that starts with `|`, with
    /// a line break or a comment before the `|`
    /// WHEN the parser reads it
    /// THEN it joins the alternatives and reports the braces (E0123) at the `{`, with one fix
    /// AND the fixed source parses, and keeps the comment
    /// AND in an arm, a line break and then `|` is still the split arm (`ArmSplit`); the message
    /// has one line (the fixture E0123-braces-pipe-line pins the text)
    #[test]
    fn a_line_break_before_a_pipe_in_braces_should_join_the_alternatives() {
        let rows = [
            in_main(&in_arm(
                "E.P {\n            x\n            | y\n        } => 1",
            )),
            in_main("    let E.P {\n        x\n        | y\n    } = e"),
            in_main(&in_arm("E.P { x\n            | y, z } => 1")),
            in_main(&in_arm("E.P { x // c\n            | y } => 1")),
            in_main(&in_arm("E.P { x\n            | y\n            | z } => 1")),
            in_main(&in_arm("E.P { x,\n            y\n            | z } => 1")),
            // Kept: a `|` at the end of the line already joined them.
            in_main(&in_arm("E.P { x |\n            y } => 1")),
        ];
        for src in rows {
            let d = parse_diagnostic(&src);
            assert_eq!(d.code(), Code::PatternForm, "{src}");
            assert!(src[d.span().range()].starts_with('{'), "{src}");
            assert!(src[..d.span().start()].ends_with("E.P "), "{src}");
            assert_eq!(d.fixes().len(), 1, "{src}");
            let fixed = apply_fix(&src, &d);
            assert!(parsed(&fixed).diagnostics.is_empty(), "{fixed}");
            if src.contains("// c") {
                assert!(fixed.contains("// c"), "{fixed}");
            }
        }
        // The fix writes the sub-pattern as written.
        let src = in_main(&in_arm(
            "E.P {\n            x\n            | y\n        } => 1",
        ));
        let fixed = apply_fix(&src, &parse_diagnostic(&src));
        assert!(fixed.contains("E.P(x\n            | y) => 1"), "{fixed}");
        // An arm that is not a sub-pattern keeps its split.
        let src = in_main(&in_arm("1\n        | 2 => 1"));
        assert_eq!(parse_diagnostic(&src).code(), Code::ArmSplit, "{src}");
        // After a sub-pattern, the arm is the place again, so the split arm stays.
        let src = in_main(&in_arm("E.P(x, y)\n        | E.Q => 1"));
        assert_eq!(parse_diagnostic(&src).code(), Code::ArmSplit, "{src}");
    }

    /// `arm` as an arm of a `match`, before the arm `None`.
    fn before_none(arm: &str) -> String {
        format!("fn main() {{\n    match o {{\n        {arm}\n        None => print(0)\n    }}\n}}")
    }

    /// GIVEN a `case` arm with a comment or a line break after `case` or `let`
    /// WHEN the parser reports the `case` (E0122)
    /// THEN the fix removes `case`, the `let` and the `.` that follow, and keeps each comment
    #[test]
    fn a_fix_of_case_should_look_past_a_line_break() {
        let rows: [(&str, &[&str]); 6] = [
            (
                "case // c\n        let .Some(x) => print(x)",
                &["case ", "let ", "."],
            ),
            (
                "case\n        let .Some(x) => print(x)",
                &["case\n        ", "let ", "."],
            ),
            (
                "case // c\n\n        let .Some(x) => print(x)",
                &["case ", "let ", "."],
            ),
            (
                "case // a\n        // b\n        let .Some(x) => print(x)",
                &["case ", "let ", "."],
            ),
            (
                "case // c\n        let Some(x) => print(x)",
                &["case ", "let "],
            ),
            (
                "case // c\n        let // d\n        .Some(x) => print(x)",
                &["case ", "let ", "."],
            ),
        ];
        for (arm, want) in rows {
            let src = before_none(arm);
            assert_eq!(removed_text(&src), want, "{src}");
            let d = parse_diagnostic(&src);
            let fixed = apply_fix(&src, &d);
            assert!(parsed(&fixed).diagnostics.is_empty(), "{fixed}");
            for comment in arm.lines().filter_map(|l| l.find("//").map(|at| &l[at..])) {
                assert!(fixed.contains(comment), "{fixed}");
            }
        }
    }

    /// GIVEN a `case` and then a line break and `=>` as an arm
    /// WHEN the parser reads it
    /// THEN `case` is a binding, as `case => x` is, and the arm is split (ArmSplit): the fix joins
    /// the lines, unless a comment lies between
    #[test]
    fn case_on_its_own_line_before_an_arrow_should_be_a_binding() {
        let rows = [
            ("case\n        => print(1)", (4, 9), 1),
            ("case // c\n        => print(1)", (4, 9), 0),
        ];
        for (arm, position, fixes) in rows {
            let src = before_none(arm);
            let d = parse_diagnostic(&src);
            assert_eq!(
                (d.code(), line_col(&src, d.span().start())),
                (Code::ArmSplit, position),
                "{src}"
            );
            assert_eq!(d.fixes().len(), fixes, "{src}");
        }
    }

    /// GIVEN `case` and then a `;` on its line, or another idiom and then a line break
    /// WHEN the parser reads the arm
    /// THEN it keeps its error, because the rule for `case` does not reach it
    #[test]
    fn an_arm_that_is_no_case_line_break_should_keep_its_error() {
        let rows = [
            ("case ; => print(1)", Code::MatchIdiom, (3, 9)),
            ("else\n        => print(1)", Code::MatchIdiom, (3, 9)),
            ("default // c\n        : print(1)", Code::Expected, (3, 21)),
        ];
        for (arm, code, position) in rows {
            let src = before_none(arm);
            let d = parse_diagnostic(&src);
            assert_eq!(
                (d.code(), line_col(&src, d.span().start())),
                (code, position),
                "{src}"
            );
        }
    }

    /// GIVEN a `case` and then a token that starts no pattern, with or without a line break
    /// AND a `case` and then a `let` first
    /// WHEN the parser reads the arm
    /// THEN `case` is a binding, or (E0122) it has a fix only when a pattern follows it
    /// AND a fix that is there gives a source that parses
    #[test]
    #[expect(
        clippy::too_many_lines,
        clippy::type_complexity,
        reason = "one row per form, so the table reads in one place"
    )]
    fn case_with_no_pattern_after_it_should_not_lose_the_arm() {
        // (arm, the one diagnostic with its position or none, fixes, the code of the fixed source)
        let rows: [(&str, Option<(Code, (usize, usize))>, usize, Option<Code>); 24] = [
            (
                "case\n        | x => print(1)",
                Some((Code::ArmSplit, (4, 9))),
                1,
                None,
            ),
            (
                "case\n        if true => print(1)",
                Some((Code::ArmSplit, (4, 9))),
                1,
                None,
            ),
            ("case | x => print(1)", None, 0, None),
            ("case if true => print(1)", None, 0, None),
            (
                "case -> print(1)",
                Some((Code::MatchIdiom, (3, 14))),
                1,
                None,
            ),
            (
                "case let => print(1)",
                Some((Code::MatchIdiom, (3, 9))),
                0,
                None,
            ),
            (
                "case\n        let => print(1)",
                Some((Code::MatchIdiom, (3, 9))),
                0,
                None,
            ),
            (
                "case let\n        | x => print(1)",
                Some((Code::MatchIdiom, (3, 9))),
                0,
                None,
            ),
            // A token that can follow no pattern in an arm: `case` is no binding, and removing it
            // leaves no pattern.
            (
                "case (x, y) => print(1)",
                Some((Code::MatchIdiom, (3, 9))),
                0,
                None,
            ),
            (
                "case(x) => print(1)",
                Some((Code::MatchIdiom, (3, 9))),
                0,
                None,
            ),
            (
                "case [x] => print(1)",
                Some((Code::MatchIdiom, (3, 9))),
                0,
                None,
            ),
            (
                "case 1.5 => print(1)",
                Some((Code::MatchIdiom, (3, 9))),
                0,
                None,
            ),
            (
                "case { x } => print(1)",
                Some((Code::MatchIdiom, (3, 9))),
                0,
                None,
            ),
            ("case: print(1)", Some((Code::MatchIdiom, (3, 9))), 0, None),
            (
                "case\n        (x) => print(1)",
                Some((Code::MatchIdiom, (3, 9))),
                0,
                None,
            ),
            // Kept: a pattern follows.
            (
                "case 1 => print(1)",
                Some((Code::MatchIdiom, (3, 9))),
                1,
                None,
            ),
            (
                "case .P(x, y) => print(1)",
                Some((Code::MatchIdiom, (3, 9))),
                1,
                None,
            ),
            (
                "case let .P(x, y) => print(1)",
                Some((Code::MatchIdiom, (3, 9))),
                1,
                None,
            ),
            (
                "case // c\n        1 => print(1)",
                Some((Code::MatchIdiom, (3, 9))),
                1,
                None,
            ),
            (
                "case ..2 => print(1)",
                Some((Code::MatchIdiom, (3, 9))),
                1,
                Some(Code::Expected),
            ),
            // Kept: a `;` ends the arm, so the fix stays. The `;` that is left is its own warning.
            (
                "case; 1 => print(1)",
                Some((Code::MatchIdiom, (3, 9))),
                1,
                Some(Code::RedundantSemicolon),
            ),
            // Kept: a binding.
            ("case => print(1)", None, 0, None),
            (
                "case\n        => print(1)",
                Some((Code::ArmSplit, (4, 9))),
                1,
                None,
            ),
            ("case if case > 0 => print(case)", None, 0, None),
        ];
        for (arm, want, fixes, fixed_code) in rows {
            let src = before_none(arm);
            let found = parsed(&src).diagnostics;
            let Some((code, position)) = want else {
                assert!(found.is_empty(), "{src}: {found:?}");
                continue;
            };
            let [d] = found.as_slice() else {
                panic!("{src}: expected one diagnostic, got {found:?}");
            };
            assert_eq!(
                (d.code(), line_col(&src, d.span().start())),
                (code, position),
                "{src}"
            );
            assert_eq!(d.fixes().len(), fixes, "{src}");
            if fixes > 0 {
                let fixed = apply_fix(&src, d);
                let after: Vec<_> = parsed(&fixed)
                    .diagnostics
                    .iter()
                    .map(Diagnostic::code)
                    .collect();
                assert_eq!(after, fixed_code.into_iter().collect::<Vec<_>>(), "{fixed}");
            }
        }
    }

    /// The text that each edit of each fix of the one error of `src` removes, with the check that
    /// no edit replaces it with text.
    fn removed_text(src: &str) -> Vec<String> {
        let d = parse_diagnostic(src);
        let mut removed = Vec::new();
        for fix in d.fixes() {
            for edit in fix.edits() {
                assert_eq!(edit.replacement(), "", "{src}");
                removed.push(src[edit.span().range()].to_string());
            }
        }
        removed
    }

    /// GIVEN each fix that removes a keyword, a label or a `mut` up to the next token, with a
    /// comment in the gap before that token
    /// WHEN the parser makes the edits of the fix
    /// THEN each edit ends where the comment starts, so the fix keeps the comment
    #[test]
    fn a_removal_should_keep_a_comment_in_its_gap() {
        let rows: [(&str, &[&str]); 9] = [
            ("fn main() {\n    let mut // c\n    _ = 1\n}", &["mut "]),
            (
                "fn main() {\n    match e {\n        E.P(x: // c\n            a, y: b) => print(a)\n    }\n}",
                &["x: ", "y: "],
            ),
            (
                // The comment touches the `:`.
                "fn main() {\n    match e {\n        E.P(x:// c\n            a, y: b) => print(a)\n    }\n}",
                &["x:", "y: "],
            ),
            ("enum class // c\nFoo {\n    A\n}", &["class "]),
            ("class A {\n    let // c\n    x: i64\n}", &["let "]),
            ("fn main() {\n    _: // c\n    loop {\n    }\n}", &["_: "]),
            (
                "fn main() {\n    match o {\n        case // c\n        Some(x) => print(x)\n    }\n}",
                &["case "],
            ),
            (
                "fn main() {\n    match o {\n        case let // c\n        .Some(x) => print(x)\n    }\n}",
                &["case ", "let ", "."],
            ),
            (
                "class A {\n    fn get(mut // c\n        self) -> i64 { 1 }\n}",
                &["mut "],
            ),
        ];
        for (src, want) in rows {
            assert_eq!(removed_text(src), want, "{src}");
        }
    }

    /// GIVEN a pattern with type arguments or labels
    /// AND a comment in the text that its fix removes: in the `<...>`, or before a label's `:`
    /// WHEN the parser reports it (E0123)
    /// THEN the diagnostic has no fix, because the fix would delete the comment
    /// AND a comment that the fix does not remove, after the `:` or the type arguments, keeps
    /// the fix
    /// AND the fixed source keeps that comment
    #[test]
    fn a_pattern_fix_should_not_delete_a_comment() {
        let rows = [
            (
                in_main(&in_arm("E<i64, // c\n            i64>.P(x, y) => print(x)")),
                (3, 10),
            ),
            (
                in_main("    if let E<i64, // c\n        i64>.P(x, y) = e { }"),
                (2, 13),
            ),
            (
                in_main(&in_arm("E.P(x // c\n            : a, y: b) => print(a)")),
                (3, 13),
            ),
            (
                in_main(&in_arm(
                    "E.P(x\n            // c\n            : a, y: b) => print(a)",
                )),
                (3, 13),
            ),
        ];
        for (src, position) in rows {
            let d = parse_diagnostic(&src);
            assert_eq!(
                (d.code(), line_col(&src, d.span().start())),
                (Code::PatternForm, position),
                "{src}"
            );
            assert!(d.fixes().is_empty(), "{src}");
        }
        let kept = [
            in_main(&in_arm("E.P(x: // c\n            a, y: b) => print(a)")),
            in_main(&in_arm(
                "E<i64, i64> // c\n            .P(x, y) => print(x)",
            )),
        ];
        for src in kept {
            let d = parse_diagnostic(&src);
            assert_eq!(d.code(), Code::PatternForm, "{src}");
            assert_eq!(d.fixes().len(), 1, "{src}");
            assert!(apply_fix(&src, &d).contains("// c"), "{src}");
        }
    }

    /// GIVEN each fix of the last test, with no comment in the gap, a comment on the line before
    /// the removed token, and a comment after the next token on its line
    /// WHEN the parser makes the edits of the fix
    /// THEN each edit reaches the next token, as it did before: a comment outside the gap changes
    /// nothing
    #[test]
    fn a_removal_with_no_comment_in_its_gap_should_reach_the_next_token() {
        let rows: [(&str, &[&str]); 8] = [
            (
                "fn main() {\n    // before\n    let mut\n    _ = 1 // later\n}",
                &["mut\n    "],
            ),
            (
                "fn main() {\n    match e {\n        // before\n        E.P(x:\n            a, y: b) => print(a) // later\n    }\n}",
                &["x:\n            ", "y: "],
            ),
            (
                "// before\nenum class\nFoo { // later\n    A\n}",
                &["class\n"],
            ),
            (
                "class A {\n    // before\n    let\n    x: i64 // later\n}",
                &["let\n    "],
            ),
            (
                "fn main() {\n    // before\n    _:\n    loop { // later\n    }\n}",
                &["_:\n    "],
            ),
            (
                "fn main() {\n    match o {\n        // before\n        case\n        Some(x) => print(x) // later\n    }\n}",
                &["case\n        "],
            ),
            (
                "fn main() {\n    match o {\n        // before\n        case let\n        .Some(x) => print(x) // later\n    }\n}",
                &["case ", "let\n        ", "."],
            ),
            (
                "class A {\n    // before\n    fn get(mut\n        self) -> i64 { 1 } // later\n}",
                &["mut\n        "],
            ),
        ];
        for (src, want) in rows {
            assert_eq!(removed_text(src), want, "{src}");
        }
    }

    /// GIVEN a `let` pattern with a `:` and no type after it
    /// WHEN the parser reads it
    /// THEN the error is the plain one of the missing type, not the one of a typed `let`
    #[test]
    fn let_pattern_with_a_colon_and_no_type_should_report_the_missing_type() {
        let src = "fn main() {\n    let Some(w): = o else { return }\n}";
        assert_eq!(parse_err(src), (Code::Expected, 2, 18));
        assert_eq!(parse_diagnostic(src).fixes(), []);
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
    fn match_arms_should_parse_each_form_to_its_tree() {
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
                "match o {\n    Option.None => 1\n    Option.Some(n) => n\n}",
                "(match o (arm Option.None 1) (arm (Option.Some n) n))",
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
    fn a_match_with_bad_syntax_should_report_the_token_found() {
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
            ("match e {\n    A { w => 1 }\n}", 2, 7),
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
    fn pattern_errors_should_report_the_code_position_and_fix() {
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
            // `||` is no prefix operator, so a `|` that starts an operand has no fix.
            (
                main("let y = |true"),
                Code::UnexpectedCharacter,
                (2, 13),
                None,
            ),
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
            // A pattern takes no type arguments; the fix removes them, whatever the pattern is.
            (
                arm("Option<i64>.Some(v) => 1"),
                Code::PatternForm,
                (3, 15),
                Some((true, arm("Option.Some(v) => 1"))),
            ),
            (
                arm("Wrap<i64>(v) => 1"),
                Code::PatternForm,
                (3, 13),
                Some((true, arm("Wrap(v) => 1"))),
            ),
            (
                arm("Option<Map<i64, string>>.Some(v) => 1"),
                Code::PatternForm,
                (3, 15),
                Some((true, arm("Option.Some(v) => 1"))),
            ),
            // A `<` that no type-argument list explains keeps the `=>` error.
            (arm("A<B => 1"), Code::Expected, (3, 10), None),
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

    /// Optionals nest, a layer for each `?`: `??` is one token of two layers, so `Tree??` and
    /// `Tree? ?` are one type. The span runs through the last `?`.
    #[test]
    fn optional_types_should_nest() {
        let rows = [
            (
                "fn main() { let x: i64?? = None }",
                "(block (let x i64?? None))",
            ),
            (
                "fn main() { let x: Tree? ? = None }",
                "(block (let x Tree?? None))",
            ),
            (
                "fn main() { let x: Stack<i64>?? = None }",
                "(block (let x Stack<i64>?? None))",
            ),
            (
                "fn main() { let x: i64??? = None }",
                "(block (let x i64??? None))",
            ),
        ];
        for (src, want) in rows {
            assert_eq!(body(src), want, "{src}");
        }
        assert_eq!(
            program("fn f(x: [i64?]??, y: Node? ?) {}"),
            "(fn f ((x [i64?]??) (y Node??)) unit (block))"
        );
        let f = first_fn("fn f() -> Tree?? {}");
        assert_eq!(f.ret.expect("has a return type").span, Span::new(10, 16));
        // After a value, `??` still coalesces.
        assert_eq!(expr("a ?? b"), "(?? a b)");
    }

    /// `any` is a keyword only before a name in a type, and a type in parentheses is that type.
    /// The expression forms read their type arguments through `scan_type`'s lookahead.
    #[test]
    fn any_types_should_parse_where_a_type_goes() {
        let rows = [
            ("let x: any Shape = y", "(let x any Shape y)"),
            ("let x: (any Shape)? = y", "(let x (any Shape)? y)"),
            ("let x: (any Shape)?? = y", "(let x (any Shape)?? y)"),
            ("let x: [(any Shape)?] = y", "(let x [(any Shape)?] y)"),
            ("let x: [any Shape] = y", "(let x [any Shape] y)"),
            ("let x: (i64) = y", "(let x i64 y)"),
            ("let x: (i64?)? = y", "(let x i64?? y)"),
            ("let x: Stack<any Shape> = y", "(let x Stack<any Shape> y)"),
            (
                "let x: any Convert<Kelvin> = y",
                "(let x any Convert<Kelvin> y)",
            ),
            // `any` with no name after it is a type named `any`.
            ("let x: any = y", "(let x any y)"),
            ("let x: [any]? = y", "(let x [any]? y)"),
            ("f<any Shape>(x)", "(call f<any Shape> x)"),
            ("f<(any Shape)?>(x)", "(call f<(any Shape)?> x)"),
            ("Stack<(any Shape)?>()", "(call Stack<(any Shape)?>)"),
            (
                "Stack<any Convert<Kelvin>>()",
                "(call Stack<any Convert<Kelvin>>)",
            ),
            // `any` after a `.` or in a value position stays a name.
            ("xs.any(f)", "(call (. xs any) f)"),
            ("let any = 1", "(let any 1)"),
            ("any + 1", "(+ any 1)"),
            // The parentheses and `any` are comparisons unless the list is a type's.
            ("f(a < any, b > c)", "(call f (< a any) (> b c))"),
            ("f(a < (b), c > (d))", "(call f (< a b) (> c d))"),
        ];
        for (src, expected) in rows {
            assert_eq!(expr(src), expected, "{src}");
        }
        // `(any Shape)?` starts at its `(` and its `?` ends it; the bare type ends at the `)`.
        let src = "fn main() { let x: (any Shape)? = y }";
        let StmtKind::Let { ty: Some(ty), .. } = &first_fn(src).body.stmts[0].kind else {
            panic!("expected a typed let");
        };
        assert_eq!(&src[ty.span.range()], "(any Shape)?");
        assert_eq!(&src[ty.bare.range()], "(any Shape)");
    }

    /// `any` binds tighter than `?` and takes one interface.
    #[test]
    fn any_written_with_a_question_mark_or_an_ampersand_should_be_any_form() {
        for (src, col) in [
            ("fn f(x: any Observer?) {}", 9),
            ("fn f(x: any Observer??) {}", 9),
            ("fn f(x: any Observer???) {}", 9),
            ("fn f(x: [any Shape?]) {}", 10),
            ("fn f(x: any Convert<Kelvin>?) {}", 9),
            ("fn f(x: any A & B) {}", 15),
            ("fn f(x: [any A & B]) {}", 16),
        ] {
            assert_eq!(parse_err(src), (Code::AnyForm, 1, col), "{src}");
        }
        for (src, fixed_src) in [
            ("fn f(x: any Observer?) {}", "fn f(x: (any Observer)?) {}"),
            ("fn f(x: any Observer??) {}", "fn f(x: (any Observer)??) {}"),
            (
                "fn f(x: any Observer???) {}",
                "fn f(x: (any Observer)???) {}",
            ),
            ("fn f(x: [any Shape?]) {}", "fn f(x: [(any Shape)?]) {}"),
        ] {
            assert_eq!(fixed(src), fixed_src, "{src}");
        }
        // Only the optional form has a fix.
        assert_eq!(parse_diagnostic("fn f(x: any A & B) {}").fixes().len(), 0);
    }

    /// Each `.Name` of a member type is a level of the tree, as each `?` is: a chain that
    /// `MAX_DEPTH` allows parses, one more is too deep, and a type argument list that holds one
    /// scans as a list only up to that limit, with its `?` layers counted too.
    #[test]
    fn member_types_should_count_toward_the_nesting_limit() {
        let codes = [MAX_DEPTH - 1, MAX_DEPTH].map(|n| {
            parsed(&format!("fn f(x: S{}) {{}}", ".A".repeat(n)))
                .diagnostics
                .iter()
                .map(Diagnostic::code)
                .collect::<Vec<_>>()
        });
        assert_eq!(codes, [vec![], vec![Code::NestingTooDeep]]);
        let list = |members: usize, layers: usize| {
            follows(&format!(
                "f<S{}{}>(x)",
                ".A".repeat(members),
                "?".repeat(layers)
            ))
        };
        assert!(follows("f<S.Item>(x)") && follows("f<Self.Item>(x)") && follows("f<Self>(x)"));
        assert!(!follows("f<S.>(x)") && !follows("f<S.1>(x)"));
        assert!(list(MAX_DEPTH - 1, 0) && !list(MAX_DEPTH, 0));
        assert!(list(MAX_DEPTH - 2, 1) && !list(MAX_DEPTH - 2, 2));
        assert!(list(1, MAX_DEPTH - 2) && !list(2, MAX_DEPTH - 2));
        // A list inside a list starts one level deeper.
        let inner = |members: usize| follows(&format!("f<Pair<S{}>>(x)", ".A".repeat(members)));
        assert!(inner(MAX_DEPTH - 2) && !inner(MAX_DEPTH - 1));
    }

    /// Each `?` is a level of the tree: a type with more layers than `MAX_DEPTH` allows is too
    /// deep, and a type argument list with them does not scan as one.
    #[test]
    fn optional_layers_should_count_toward_the_nesting_limit() {
        let deep = |n: usize| format!("fn main() {{ let x: i64{} = None }}", "?".repeat(n));
        let codes = [MAX_DEPTH - 8, MAX_DEPTH + 1].map(|n| {
            parsed(&deep(n))
                .diagnostics
                .iter()
                .map(Diagnostic::code)
                .collect::<Vec<_>>()
        });
        assert_eq!(codes, [vec![], vec![Code::NestingTooDeep]]);
        assert!(follows(&format!("f<i64{}>(x)", "?".repeat(MAX_DEPTH - 1))));
        assert!(!follows(&format!("f<i64{}>(x)", "?".repeat(MAX_DEPTH))));
        assert!(follows("f<i64??>(x)"));
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
    fn array_and_for_syntax_errors_should_report_their_code_and_position() {
        let rows = [
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
    fn range_should_parse_in_each_form() {
        let forms = [
            ("a..b", "(.. a b)"),
            ("a..=b", "(..= a b)"),
            ("a..", "(.. a _)"),
            ("..b", "(.. _ b)"),
            ("..=b", "(..= _ b)"),
            ("..", "(.. _ _)"),
            // Looser than every operator.
            ("a + 1..b * 2", "(.. (+ a 1) (* b 2))"),
            ("a == b..c", "(.. (== a b) c)"),
        ];
        for (range, want) in forms {
            assert_eq!(expr(&format!("f({range})")), format!("(call f {want})"));
            assert_eq!(expr(&format!("xs[{range}]")), format!("(index xs {want})"));
        }
    }

    #[test]
    fn brace_after_dots_should_start_the_body_in_each_header() {
        let rows = [
            ("for i in 0.. { }", "(for i (.. 0 _) (block))"),
            ("if x == 0.. { }", "(if (.. (== x 0) _) (block))"),
            ("while x == 0.. { }", "(while (.. (== x 0) _) (block))"),
            ("match x.. { }", "(match (.. x _))"),
            (
                "if let Some(y) = x.. { }",
                "(if-let (Some y) (.. x _) (block))",
            ),
            (
                "while let Some(y) = x.. { }",
                "(while-let (Some y) (.. x _) (block))",
            ),
        ];
        for (src, want) in rows {
            assert_eq!(expr(src), want, "{src}");
        }
    }

    #[test]
    fn brace_after_dots_should_be_an_end_inside_brackets() {
        let rows = [
            (
                "if xs[0..{ 2 }].len() == 2 { }",
                "(if (== (call (. (index xs (.. 0 (block 2))) len)) 2) (block))",
            ),
            (
                "while f(0..{ 2 }) { }",
                "(while (call f (.. 0 (block 2))) (block))",
            ),
            (
                "let r = xs[0..{ 2 }]",
                "(let r (index xs (.. 0 (block 2))))",
            ),
            (
                "if { let r = 0..{ 2 }; true } { }",
                "(if (block (let r (.. 0 (block 2))) true) (block))",
            ),
        ];
        for (src, want) in rows {
            assert_eq!(expr(src), want, "{src}");
        }
    }

    /// An interpolation is a bracket: a `{` after dots in it is an end, and a block in it is read
    /// as in any other bracket, inside a header too.
    #[test]
    fn brace_after_dots_should_be_an_end_inside_an_interpolation_in_a_header() {
        let rows = [
            (
                r#"if s == "{0..{ 1 }}" { }"#,
                r"(if (== s (interp (.. 0 (block 1)))) (block))",
            ),
            (
                r#"if s == "{ { 1 } }" { }"#,
                r"(if (== s (interp (block 1))) (block))",
            ),
            (
                r#"for i in 0.."{ { 1 } }".len() { }"#,
                r"(for i (.. 0 (call (. (interp (block 1)) len))) (block))",
            ),
        ];
        for (src, want) in rows {
            assert_eq!(expr(src), want, "{src}");
        }
    }

    /// The header flag is restored when a header fails to parse, so the next item reads a block
    /// as an end again.
    #[test]
    fn brace_after_dots_should_be_an_end_after_an_error_in_a_header() {
        let parsed = parsed("fn f() { for i in 0..* {} }\nfn main() { let r = 0..{ 2 } }");
        let [d] = parsed.diagnostics.as_slice() else {
            panic!("expected one diagnostic, got {:?}", parsed.diagnostics);
        };
        assert_eq!(
            line_col("fn f() { for i in 0..* {} }", d.span().start()),
            (1, 22)
        );
    }

    #[test]
    fn range_end_should_keep_the_literal_error() {
        assert_eq!(
            expr_err("f(0..9223372036854775808)"),
            (Code::IntegerTooLarge, 1, 18)
        );
    }

    /// GIVEN a range pattern with one bound, a literal or a name, in each place that has patterns
    /// WHEN the parser reads it
    /// THEN it reports one error (E0011) over the operator and the bound, with no fix
    /// AND a range whose end is no bound keeps its error, and a foreign operator is E0132
    /// AND the fixtures E0011-range-pattern-no-start, -no-end and -forms pin the text
    #[test]
    fn a_range_pattern_with_one_bound_should_name_the_missing_bound() {
        let arm = |arm: &str| {
            format!(
                "fn f(e: E, n: i64, c: char, lo: i64) {{\n    match n {{\n        {arm}\n        _ => print(2)\n    }}\n}}"
            )
        };
        let body = |stmts: &str| format!("fn f(x: i64) {{\n{stmts}\n}}");
        // (source, code, the text under the span)
        let rows: Vec<(String, Code, &str)> = vec![
            (arm("..2 => print(1)"), Code::Expected, "..2"),
            (arm("..=2 => print(1)"), Code::Expected, "..=2"),
            (arm("5.. => print(1)"), Code::Expected, "5.."),
            (arm("5..= => print(1)"), Code::Expected, "5..="),
            (arm(".. => print(1)"), Code::Expected, ".."),
            (arm("1 | ..2 => print(1)"), Code::Expected, "..2"),
            (arm("5.. | 7 => print(1)"), Code::Expected, "5.."),
            (arm("5.. if true => print(1)"), Code::Expected, "5.."),
            (arm("5..\n        => print(1)"), Code::Expected, "5.."),
            (arm("E.P(..2, y) => print(1)"), Code::Expected, "..2"),
            (arm("E.P(5.., y) => print(1)"), Code::Expected, "5.."),
            (arm("E.P(x, 5..) => print(1)"), Code::Expected, "5.."),
            (arm("'a'.. => print(1)"), Code::Expected, "'a'.."),
            (arm("..'z' => print(1)"), Code::Expected, "..'z'"),
            (arm("lo.. => print(1)"), Code::Expected, "lo.."),
            (arm("..lo => print(1)"), Code::Expected, "..lo"),
            (arm("-5.. => print(1)"), Code::Expected, "-5.."),
            (arm("..-5 => print(1)"), Code::Expected, "..-5"),
            (
                body("    let 5.. = x else { return }"),
                Code::Expected,
                "5..",
            ),
            (body("    if let ..2 = x { }"), Code::Expected, "..2"),
            (body("    while let 5.. = x { }"), Code::Expected, "5.."),
            // A bound that is no literal or name: the range has the one bound that is there.
            (arm("..foo(1) => print(1)"), Code::Expected, ".."),
            (arm("..- => print(1)"), Code::Expected, ".."),
            (arm(".. <5 => print(1)"), Code::Expected, ".."),
            // Unchanged: the end is a call, so the literal error names it.
            (arm("5..foo(1) => print(1)"), Code::Expected, "foo"),
            // A foreign operator with no start is E0132, as after a start.
            (arm("..<5 => print(1)"), Code::ForeignRange, "..<"),
            (arm("...5 => print(1)"), Code::ForeignRange, "..."),
            // Unchanged.
            (arm("5..< => print(1)"), Code::ForeignRange, "..<"),
            (arm("5... => print(1)"), Code::ForeignRange, "..."),
        ];
        for (src, code, under) in rows {
            let d = parse_diagnostic(&src);
            assert_eq!(d.code(), code, "{src}");
            assert_eq!(&src[d.span().range()], under, "{src}");
            assert!(d.fixes().is_empty(), "{src}");
        }
    }

    /// `xs[1:3]` in Python and Go: each form is one E0132 at the `:`, with a fix, and the range it
    /// names for the checker. Only `xs[:]` has a safe fix: the parser cannot see the type of the
    /// base, so `..` may count bytes, and may stop the program, where Python clamps. A step is
    /// E0011.
    #[test]
    fn colon_slice_should_recover_as_the_range_it_names() {
        let rows = [
            ("xs[1:3]", (1, 17), "(index xs (.. 1 3))", "xs[1..3]"),
            ("xs[1:]", (1, 17), "(index xs (.. 1 _))", "xs[1..]"),
            ("xs[:3]", (1, 16), "(index xs (.. _ 3))", "xs[..3]"),
            ("xs[:]", (1, 16), "xs", "xs"),
            // A zero bound is not negative.
            ("xs[:0]", (1, 16), "(index xs (.. _ 0))", "xs[..0]"),
            ("xs[0:]", (1, 17), "(index xs (.. 0 _))", "xs[0..]"),
            (
                "f()[1:n]",
                (1, 18),
                "(index (call f) (.. 1 n))",
                "f()[1..n]",
            ),
        ];
        for (src, at, tree, fixed) in rows {
            let program = format!("fn main() {{ {src} }}");
            let parsed = parsed(&program);
            let [d] = parsed.diagnostics.as_slice() else {
                panic!(
                    "{src}: expected one diagnostic, got {:?}",
                    parsed.diagnostics
                );
            };
            assert_eq!(d.code(), Code::ForeignRange, "{src}");
            assert_eq!(line_col(&program, d.span().start()), at, "{src}");
            let (safe, _) = fix_edits(d).expect("the colon form has a fix");
            assert_eq!(safe, src == "xs[:]", "{src}");
            let fixed = format!("fn main() {{ {fixed} }}");
            assert_eq!(apply_fix(&program, d), fixed);
            // Each fix gives a range that parses with no diagnostic.
            assert_eq!(parsed_fixed(&fixed), 0, "{src}");
            let Some(Item::Function(f)) = parsed.program.items.first() else {
                panic!("{src}: expected a function");
            };
            assert_eq!(f.body.stmts[0].to_string(), tree, "{src}");
        }
        // A step, and a bound that is a range, which `..` would chain, have no colon form.
        for src in [
            "xs[::2]",
            "xs[1:2:3]",
            "xs[1::2]",
            "xs[0:1..2]",
            "xs[0..1:2]",
            "xs[0..1:]",
            "xs[:0..=2]",
            "xs[0..1:2..3]",
            "xs[(0..1):2]",
        ] {
            assert_eq!(expr_err(src).0, Code::Expected, "{src}");
        }
    }

    /// Python counts a negative colon bound from the end, and Sisu has no negative index: the
    /// report has no fix, and no rewrite, because the parser cannot see the type of the base (a
    /// string counts bytes) or know that a bound does not change the base. The wording is pinned
    /// in `tests/diagnostics/E0132-colon-negative.sisu`.
    #[test]
    fn colon_slice_with_a_negative_bound_should_have_no_fix() {
        for src in [
            "xs[:-1]",
            "xs[-2:]",
            "xs[-3:-1]",
            "s[:-1]",
            "xs[:-n]",
            "xs[:-(n + 1)]",
            "xs[:(-1)]",
            "xs[1:-1]",
            "c.xs[:-1]",
            "xss[0][:-1]",
            "xs[:-a * b]",
            "xs[:-1 + n]",
            "xs[:-1 < n]",
            "xs[:--1]",
            "xs[-3:-a * b]",
            "xs[:-9223372036854775808]",
            "xs[{ xs = [7]; 0 }:-1]",
            "self.xs[:-1]",
            "f()[:-1]",
            "f().xs[:-1]",
            "xs[1..][:-1]",
        ] {
            let d = parse_diagnostic(&format!("fn main() {{ {src} }}"));
            assert_eq!(d.code(), Code::ForeignRange, "{src}");
            assert!(d.fixes().is_empty(), "{src}");
        }
    }

    /// Python counts a negative colon bound from the end, and Sisu has no negative index: the
    /// report points at the first negative bound, with its whole span, not at the `:`. The
    /// table gives each row's column and length on line 1, after `fn main() { ` (12 characters).
    #[test]
    fn colon_slice_with_a_negative_bound_should_point_at_the_bound() {
        let rows = [
            ("xs[:-1]", 17, 2),
            ("xs[-2:]", 16, 2),
            ("xs[-3:-1]", 16, 2),
            ("xs[-2:-1]", 16, 2),
            ("s[:-1]", 16, 2),
            ("xs[:-n]", 17, 2),
            ("xs[:-(n + 1)]", 17, 8),
            ("xs[:(-1)]", 17, 4),
            ("xs[1:-1]", 18, 2),
            ("xs[0:--1]", 18, 3),
            ("c.xs[:-1]", 19, 2),
            ("xss[0][:-1]", 21, 2),
            ("xs[:-a * b]", 17, 6),
            ("xs[:-1 + n]", 17, 6),
            ("xs[:-1 < n]", 17, 6),
            ("xs[:--1]", 17, 3),
            ("xs[-3:-a * b]", 16, 2),
            ("xs[:-9223372036854775808]", 17, 20),
            ("xs[{ xs = [7]; 0 }:-1]", 32, 2),
            ("self.xs[:-1]", 22, 2),
            ("f()[:-1]", 18, 2),
            ("f().xs[:-1]", 21, 2),
            ("xs[1..][:-1]", 22, 2),
            // A bound on two lines: the span covers both.
            ("xs[1:-f(\n 1)]", 18, 7),
        ];
        for (src, col, len) in rows {
            let program = format!("fn main() {{ {src} }}");
            let d = parse_diagnostic(&program);
            assert_eq!(d.code(), Code::ForeignRange, "{src}");
            let (start, end) = (d.span().start(), d.span().end());
            assert_eq!(line_col(&program, start), (1, col), "{src}");
            assert_eq!(end - start, len, "{src}");
        }
    }

    /// The text of an E0123 message goes on one line, but the replacement of its fix is the source
    /// as written: a comment inside a sub-pattern, and its line break, stay. The wording of the
    /// messages is pinned in `tests/diagnostics/E0123-multiline.sisu`.
    #[test]
    fn braces_pattern_fix_should_keep_its_replacement_as_written() {
        let program = "fn main() {\n    match r {\n        R { w: Some( // keep me\n            a), h } => print(h)\n        _ => print(0)\n    }\n}";
        let d = parse_diagnostic(program);
        assert_eq!(d.code(), Code::PatternForm);
        let (safe, edits) = fix_edits(&d).expect("the braces form has a fix");
        assert!(!safe);
        let [(start, end, replacement)] = edits.as_slice() else {
            panic!("expected one edit, got {edits:?}");
        };
        assert_eq!(replacement, "(Some( // keep me\n            a), h)");
        assert_eq!(
            &program[*start..*end],
            " { w: Some( // keep me\n            a), h }"
        );
    }

    /// The number of diagnostics of `program`.
    fn parsed_fixed(program: &str) -> usize {
        parsed(program).diagnostics.len()
    }

    fn statement_place(src: &str) -> bool {
        let f = first_fn(&format!("fn main() {{ {src} }}"));
        let StmtKind::Expr(e) = &f.body.stmts[0].kind else {
            panic!("{src}: expected an expression");
        };
        is_place(e)
    }

    #[test]
    fn range_index_should_not_be_a_place() {
        assert!(!statement_place("xs[1..3]"));
        assert!(!statement_place("xs[1..][0]"));
        // A field of an element of a range index is the shared object's, as for a class element.
        assert!(statement_place("xs[..1][0].n"));
        assert!(statement_place("xs[0]"));
    }

    #[test]
    fn range_target_should_parse_as_an_assignment() {
        let rows = [
            ("xs[0..2] = [7, 8]", "(= (index xs (.. 0 2)) (array 7 8))"),
            ("xs[1..][0] += 5", "(+= (index (index xs (.. 1 _)) 0) 5)"),
            ("s[0..=1] += \"x\"", "(+= (index s (..= 0 1)) \"x\")"),
            (
                "xs[..1][0].n = 9",
                "(= (. (index (index xs (.. _ 1)) 0) n) 9)",
            ),
        ];
        for (src, want) in rows {
            assert_eq!(expr(src), want, "{src}");
        }
        assert_eq!(expr_err("f() = 1").0, Code::AssignToExpression);
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
            ("interface I {};", (1, 15), 14, ""),
            ("extend C {};", (1, 12), 11, ""),
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
            // A `>` that closes a field's type arguments ends the field, as `i64` does.
            "class D { c: Box<i64>; n: i64 }",
            "class D { c: Box<Box<i64>>; n: i64 }",
            "fn main() { let b: Box<i64> = x; let n = 1 }",
        ] {
            parse_ok(src);
        }
        // At a line's end the `;` is redundant there, as after `i64`.
        let src = "class D {\n    c: Box<i64>;\n    n: i64\n}";
        let codes: Vec<_> = parsed(src)
            .diagnostics
            .iter()
            .map(Diagnostic::code)
            .collect();
        assert_eq!(codes, [Code::RedundantSemicolon], "{src}");
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
            // A `>` that compares does not end a statement, nor one that closes a header's list.
            ("fn main() { let x = a >; }", 24),
            ("fn f<T>; (x: T) {}", 8),
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::Expected, "{src}");
            assert_eq!(line_col(src, d.span().start()), (1, col), "{src}");
            assert!(d.help().is_none(), "{src}");
            assert_eq!(fix_edits(&d), None, "{src}");
        }
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
    fn reserved_words_should_be_errors_where_a_name_goes() {
        // One row per place that wants a name or an expression.
        for (src, line, col) in [
            ("fn main() {\n    let weak = 1\n}", 2, 9),
            ("class C {\n    move: i64\n}", 2, 5),
            ("fn main() {\n    print(p.weak)\n}", 2, 13),
            ("import x", 1, 1),
        ] {
            assert_eq!(parse_err(src), (Code::ReservedWord, line, col), "{src}");
        }
    }

    #[test]
    fn echo_should_parse_as_a_name() {
        parse_ok("fn main() {\n    let echo = 1\n    print(echo)\n}");
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
    fn mut_before_a_reserved_word_should_be_expected() {
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
    fn mut_self_fix_should_be_safe_only_when_self_ends_the_parameter() {
        // Safe only when `self` ends the parameter. A type that does not parse, or a token other
        // than `,` or `)` after the type, keeps the one edit that removes `mut `.
        for (src, edit, safe) in [
            ("class C {\n    fn f(mut self) {}\n}", (19, 23), true),
            (
                "class C {\n    fn f(mut   self, n: i64) {}\n}",
                (19, 25),
                true,
            ),
            ("class C {\n    fn f(mut self: ) {}\n}", (19, 23), false),
            (
                "class C {\n    fn f(mut self: i64 x) {}\n}",
                (19, 23),
                false,
            ),
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
        // A typed receiver also loses its type, as the parser refuses a type on `self`.
        let typed = "class C {\n    fn f(mut self: i64) {}\n}";
        let d = parse_diagnostic(typed);
        assert_eq!(
            fix_edits(&d),
            Some((
                false,
                vec![(19, 23, String::new()), (27, 32, String::new())]
            ))
        );
    }

    /// GIVEN a `mut self` with a type, in a class, an enum or an interface
    /// WHEN the fix is applied
    /// THEN the `mut` and the whole type are gone, and the source has no syntax error
    #[test]
    fn a_typed_receiver_fix_should_remove_the_mut_and_the_type() {
        for (src, line, want) in [
            (
                "class C {\n    fn f(mut self: i64) {}\n}",
                2,
                "class C {\n    fn f(self) {}\n}",
            ),
            (
                "class C {\n    fn f(mut self: i64, a: i64) {}\n}",
                2,
                "class C {\n    fn f(self, a: i64) {}\n}",
            ),
            (
                "enum E {\n    A\n    fn f(mut self: i64) {}\n}",
                3,
                "enum E {\n    A\n    fn f(self) {}\n}",
            ),
            (
                "interface I {\n    fn f(mut self: i64)\n}",
                2,
                "interface I {\n    fn f(self)\n}",
            ),
            (
                "class C {\n    fn f(mut self: Box<i64>) {}\n}",
                2,
                "class C {\n    fn f(self) {}\n}",
            ),
            (
                "class C {\n    fn f(mut self: [i64]?) {}\n}",
                2,
                "class C {\n    fn f(self) {}\n}",
            ),
            (
                "class C {\n    fn f(mut self: (i64)) {}\n}",
                2,
                "class C {\n    fn f(self) {}\n}",
            ),
            (
                "class C {\n    fn f(mut self: (Box<i64>)) {}\n}",
                2,
                "class C {\n    fn f(self) {}\n}",
            ),
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::MutSelf, "{src}");
            assert_eq!(line_col(src, d.span().start()), (line, 10), "{src}");
            assert!(
                matches!(d.fixes(), [fix] if !fix.safe()),
                "{src}: {:?}",
                d.fixes()
            );
            let result = apply_fix(src, &d);
            assert_eq!(result, want, "{src}");
            assert!(parsed(&result).diagnostics.is_empty(), "{result}");
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

    /// GIVEN `struct P { mut x: i64 }`
    /// WHEN it is parsed
    /// THEN the one diagnostic is E0175 at 1:12, and its safe fix gives `struct P { x: i64 }`
    #[test]
    fn struct_field_with_mut_should_report_its_word_and_a_safe_fix() {
        let src = "struct P { mut x: i64 }";
        let d = parse_diagnostic(src);
        assert_eq!(d.code(), Code::StructFieldMut);
        assert_eq!(line_col(src, d.span().start()), (1, 12));
        assert_eq!(apply_fix(src, &d), "struct P { x: i64 }");
        assert!(fix_edits(&d).is_some_and(|(safe, _)| safe));
    }

    #[test]
    fn mut_self_outside_the_receiver_position() {
        assert_eq!(parse_err("fn f(mut self) {}"), (Code::SelfNotFirst, 1, 10));
        assert_eq!(
            parse_err("class C {\n    fn f(self, n: i64, mut self) {}\n}"),
            (Code::SelfNotFirst, 2, 28)
        );
    }

    /// GIVEN `fn f(mut xs: [i64], n: i64) {}`
    /// WHEN it is parsed
    /// THEN the first parameter is `mut`, the second is not, and nothing is reported
    #[test]
    fn mut_parameter_should_parse() {
        let f = first_fn("fn f(mut xs: [i64], n: i64) {}");
        let mutable: Vec<_> = f.params.iter().map(|p| p.mutable).collect();
        assert_eq!(mutable, [true, false]);
    }

    #[test]
    fn mut_before_neither_self_nor_a_name_should_be_expected() {
        assert_eq!(parse_err("fn f(mut 1) {}"), (Code::Expected, 1, 6));
    }

    /// The label, the `mut` keyword and the value of the only argument of the call in `main`.
    fn only_arg(src: &str) -> (Option<String>, Option<Span>, String) {
        let function = first_fn(src);
        let StmtKind::Expr(Expr {
            kind: ExprKind::Call { args, .. },
            ..
        }) = &function.body.stmts[0].kind
        else {
            panic!("{src}: the first statement should be a call");
        };
        let [arg] = args.as_slice() else {
            panic!("{src}: the call should have one argument");
        };
        (
            arg.label.as_ref().map(|l| l.name.as_str().to_string()),
            arg.mut_keyword,
            arg.value.to_string(),
        )
    }

    /// GIVEN a call with `mut` before the value, after the label and before the label
    /// WHEN the parser reads it
    /// THEN `mut_keyword` is the span of the keyword, with the label and the value as written
    #[test]
    fn mut_argument_should_parse_in_each_form() {
        for (call, label, value) in [
            ("f(mut b.cells)", None, "(. b cells)"),
            ("f(cells: mut xs)", Some("cells"), "xs"),
            ("f(mut cells: xs)", Some("cells"), "xs"),
        ] {
            let src = format!("fn main() {{ {call} }}");
            let keyword = src.find("mut").expect("the call has a `mut`");
            let (got_label, got_keyword, got_value) = only_arg(&src);
            assert_eq!(got_label.as_deref(), label, "{call}");
            assert_eq!(got_keyword, Some(Span::new(keyword, keyword + 3)), "{call}");
            assert_eq!(got_value, value, "{call}");
        }
        assert_eq!(only_arg("fn main() { f(xs) }").1, None);
    }

    /// GIVEN a call with a `mut` argument, with and without a label
    /// WHEN the program prints
    /// THEN the argument shows its `mut`
    #[test]
    fn mut_argument_should_print_with_its_mut() {
        assert_eq!(
            body("fn main() { f(mut xs, a: mut ys) }"),
            "(block (call f (mut xs) (: a (mut ys))))"
        );
    }

    /// GIVEN `mut` in front of a `let` initializer, and in front of other operands, with spaces,
    /// tabs, a comment, a line break and the end of the file after it
    /// WHEN the parser reads it
    /// THEN it reports E0168 at the keyword, once, with the binding help for a `let` with no `mut`
    /// AND the safe fix removes `mut` and the spaces and tabs after it, not a comment or a break
    #[test]
    fn mut_outside_an_argument_should_be_reported() {
        for (src, fixed, binding) in [
            (
                "fn main() {\n    let y = mut   x\n}",
                "fn main() {\n    let y = x\n}",
                true,
            ),
            (
                "fn main() {\n    let y = mut\t\tx\n}",
                "fn main() {\n    let y = x\n}",
                true,
            ),
            (
                "fn main() {\n    let y: i64 = mut x\n}",
                "fn main() {\n    let y: i64 = x\n}",
                true,
            ),
            (
                "fn main() {\n    let Some(y) = mut x else { return }\n}",
                "fn main() {\n    let Some(y) = x else { return }\n}",
                true,
            ),
            // `let mut` already binds a variable, and a block `const` has no `let`.
            (
                "fn main() {\n    let mut y = mut x\n}",
                "fn main() {\n    let mut y = x\n}",
                false,
            ),
            (
                "fn main() {\n    const X = mut 1\n}",
                "fn main() {\n    const X = 1\n}",
                false,
            ),
            ("const X = mut 1\n", "const X = 1\n", false),
            ("const X = mut", "const X = ", false),
            (
                "fn main() {\n    return mut x\n}",
                "fn main() {\n    return x\n}",
                false,
            ),
            (
                "fn main() {\n    let y = -mut(x)\n}",
                "fn main() {\n    let y = -(x)\n}",
                false,
            ),
            // A comment and a line break stay: the fix removes `mut ` alone.
            (
                "fn main() {\n    let y = mut // keep\n    x\n}",
                "fn main() {\n    let y = // keep\n    x\n}",
                true,
            ),
            (
                "fn main() {\n    return mut\n}",
                "fn main() {\n    return \n}",
                false,
            ),
            (
                "fn main() {\r\n    let y = mut \r\n x\r\n}",
                "fn main() {\r\n    let y = \r\n x\r\n}",
                true,
            ),
        ] {
            let d = parse_diagnostic(src);
            assert_eq!(d.code(), Code::MutNotArgument, "{src}");
            // The last `mut`, which is the one that no argument owns.
            let keyword = src.rfind("mut").expect("the program has a `mut`");
            assert_eq!(d.span(), Span::new(keyword, keyword + 3), "{src}");
            assert_eq!(d.help().is_some(), binding, "{src}");
            assert_eq!(apply_fix(src, &d), fixed, "{src}");
            assert!(fix_edits(&d).is_some_and(|(safe, _)| safe), "{src}");
        }
    }

    /// GIVEN a `mut` that starts a statement, in a function and in a block
    /// WHEN the parser reads it
    /// THEN it reports E0011 at the keyword, with no fix: it may be a declaration with no `let`
    #[test]
    fn mut_at_a_statement_start_should_be_expected_with_no_fix() {
        for src in [
            "fn main() {\n    mut x = 1\n}",
            "fn main() {\n    mut x\n}",
            "fn main() {\n    mut x.f = 1\n}",
            "fn main() {\n    if true {\n        mut x = 1\n    }\n}",
        ] {
            let d = parse_diagnostic(src);
            let keyword = src.find("mut").expect("the program has a `mut`");
            assert_eq!(d.code(), Code::Expected, "{src}");
            assert_eq!(d.span(), Span::new(keyword, keyword + 3), "{src}");
            assert_eq!(fix_edits(&d), None, "{src}");
        }
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

    /// GIVEN a struct with a `mut` field and a struct with a syntax error
    /// WHEN the parser reads them
    /// THEN the first is a struct item that keeps its `struct` keyword and reads the field as
    /// not mutable. The second keeps its name as a broken struct
    #[test]
    fn struct_items_should_parse_like_classes() {
        let parsed = parsed("struct   P { mut x: i64 }\nstruct Q {\n    y i64\n}\n");
        assert_eq!(parsed.program.to_string(), "(struct P (x i64))");
        let [Item::Struct(p)] = parsed.program.items.as_slice() else {
            panic!("expected struct P alone, found {:?}", parsed.program.items);
        };
        assert_eq!(p.keyword, Span::new(0, 6));
        let [Broken { name, kind }] = parsed.program.broken.as_slice() else {
            panic!(
                "expected one broken item, found {:?}",
                parsed.program.broken
            );
        };
        assert_eq!((name.name.as_str(), *kind), ("Q", BrokenKind::Struct));
    }

    #[test]
    fn earliest_error_in_an_item_should_win() {
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
        // The parser no longer reports a pattern with no `else`, so the lexer's error, at the
        // first `_`, stands alone.
        assert_eq!(
            parse_err("fn main() {\n    let 1__2 = 1\n}\n"),
            (Code::DigitSeparator, 2, 10)
        );
        // A pattern with no `else` is the checker's, which does not run on a broken item: the
        // parse error after it shows alone.
        assert_eq!(
            parse_err("fn main() {\n    let One(n) = w\n    let = 1\n}\n"),
            (Code::Expected, 3, 9)
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
