//! Literals: numbers (decimal, and `0x`, `0b` and `0o` with digit separators), `char` and
//! string literals, and the escapes in them.

use std::fmt;

use super::{Chars, MIN_MAGNITUDE, Token, TokenKind, literal_too_large, take_while};
use crate::diagnostic::{Diagnostic, DiagnosticKind, LiteralKind};
use crate::source::Span;

/// The number or `char` literal that starts at `source[start..]`, whose first char `chars` has
/// consumed, after the `tokens` before it. After an error it records the diagnostic and gives a
/// placeholder token, so that lexing and parsing go on.
pub(super) fn token(
    source: &str,
    start: usize,
    chars: &mut Chars,
    diagnostics: &mut Vec<Diagnostic>,
    tokens: &[Token],
) -> TokenKind {
    let (literal, placeholder) = if source[start..].starts_with('\'') {
        (char_literal(source, start, chars), TokenKind::Char('\0'))
    } else {
        // A `;` between is dropped later, so it must not change how the digits lex.
        let after_dot = tokens
            .iter()
            .rfind(|t| t.kind != TokenKind::Semicolon)
            .is_some_and(|t| matches!(t.kind, TokenKind::Dot | TokenKind::QuestionDot));
        (number(source, start, chars, after_dot), TokenKind::Int(0))
    };
    literal.unwrap_or_else(|d| {
        diagnostics.push(d);
        placeholder
    })
}

/// A float literal's value. It is never NaN, so equality by bits is equality of values and `Eq`
/// holds.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Float(pub(crate) f64);

impl PartialEq for Float {
    fn eq(&self, other: &Float) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Float {}

impl fmt::Display for Float {
    /// The shortest text that reads back as the same `f64`, always with a `.` or an exponent.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

/// The literal that starts at the digit `source[start..]`, whose first char `chars` has consumed.
/// A prefixed literal takes every ASCII letter, digit and `_` after its prefix, so a wrong digit
/// is reported at that digit and not as the start of an identifier. A decimal one is a float when
/// a `.` and a digit, or an exponent, follow its digits, unless `after_dot`: a digit run right
/// after a `.` is an integer, so that `t.0.1` reads two elements.
fn number(
    source: &str,
    start: usize,
    chars: &mut Chars,
    after_dot: bool,
) -> Result<TokenKind, Diagnostic> {
    let prefix = if source[start..].starts_with('0') {
        chars.next_if(|&(_, c)| matches!(c, 'x' | 'X' | 'b' | 'B' | 'o' | 'O'))
    } else {
        None
    };
    let (radix, digits_at, end) = match prefix {
        Some((_, p)) => {
            let radix = match p.to_ascii_lowercase() {
                'x' => 16,
                'b' => 2,
                _ => 8,
            };
            let end = take_while(chars, source, |c| c.is_ascii_alphanumeric() || c == '_');
            (radix, start + 2, end)
        }
        None => (10, start, take_while(chars, source, is_digit_or_separator)),
    };
    let whole = Span::new(start, end);
    if let Some((_, prefix)) = prefix.filter(|_| digits_at == end) {
        return Err(DiagnosticKind::BasePrefixWithoutDigits { prefix, at: whole }.into());
    }
    // Everything that belongs to the literal is consumed before the first error is reported, so
    // that one error leaves nothing of it to lex.
    let separators = check_digits(source, digits_at, end, radix);
    let float = if prefix.is_none() && !after_dot {
        float_rest(source, start, end, chars)
    } else {
        Ok(None)
    };
    separators?;
    if let Some(float) = float? {
        return Ok(float);
    }
    let digits = &source[digits_at..end];
    let value = u64::from_str_radix(&digits.replace('_', ""), radix)
        .ok()
        .filter(|&n| n <= MIN_MAGNITUDE);
    // Digits right after a `.` are a float's fraction or a field name, never an integer's value,
    // and the parser reports both, so a run too large for one is no error here.
    // shortcut: the token holds 0, so element access (`t.0`) must report a too-large index itself.
    let value = match value {
        Some(n) => n,
        None if after_dot => return Ok(TokenKind::Int(0)),
        None => return Err(literal_too_large(whole)),
    };
    // Reported last, so that its fix leaves a literal that lexes. A bare 2^63 still fails to
    // parse; only a `-` before it makes it valid, which the lexer cannot see.
    if let Some((_, p)) = prefix.filter(|(_, p)| p.is_ascii_uppercase()) {
        let at = Span::new(start, start + 2);
        let safe = value != MIN_MAGNITUDE;
        return Err(DiagnosticKind::BasePrefixUpperCase {
            prefix: p,
            safe,
            at,
        }
        .into());
    }
    Ok(TokenKind::Int(value))
}

fn is_digit_or_separator(c: char) -> bool {
    c.is_ascii_digit() || c == '_'
}

/// The first error in the digit run `source[at..end]` of base `radix`: a `_` that does not sit
/// between two letters or digits of the run, or a letter or digit that is not of the base.
fn check_digits(source: &str, at: usize, end: usize, radix: u32) -> Result<(), Diagnostic> {
    let digits = &source[at..end];
    for (i, c) in digits.char_indices() {
        let at = at + i;
        let neighbor = |j: Option<usize>| {
            j.and_then(|j| digits.as_bytes().get(j))
                .is_some_and(u8::is_ascii_alphanumeric)
        };
        if c == '_' {
            if !neighbor(i.checked_sub(1)) || !neighbor(Some(i + 1)) {
                return Err(DiagnosticKind::DigitSeparator {
                    at: Span::new(at, at + 1),
                }
                .into());
            }
        } else if !c.is_digit(radix) {
            return Err(DiagnosticKind::DigitOutsideBase {
                digit: c,
                radix,
                at: Span::new(at, at + 1),
            }
            .into());
        }
    }
    Ok(())
}

/// What follows the decimal digits `source[start..end]`, whose last char `chars` has consumed:
/// the fraction and the exponent of a float, which `chars` then consumes too, or `None` when the
/// number is an integer. A `.` ends the integer when a `.` or the start of a name follows it, as
/// in `0..n` and `1.max(2)`; any other char after it is a float that lacks its digits.
fn float_rest(
    source: &str,
    start: usize,
    end: usize,
    chars: &mut Chars,
) -> Result<Option<TokenKind>, Diagnostic> {
    let mut float_end = end;
    let mut fraction = None;
    if source[end..].starts_with('.') {
        let after = source[end + 1..].chars().next();
        match after {
            Some(c) if c.is_ascii_digit() => {
                chars.next();
                float_end = take_while(chars, source, is_digit_or_separator);
                fraction = Some(end + 1);
            }
            Some(c) if c == '.' || c.is_ascii_alphabetic() || c == '_' => {}
            _ => {
                chars.next();
                return Err(DiagnosticKind::FloatNeedsDigits {
                    at: Span::new(end, end + 1),
                    fix: Span::new(end + 1, end + 1),
                }
                .into());
            }
        }
    }
    let exponent = chars
        .next_if(|&(_, c)| matches!(c, 'e' | 'E'))
        .map(|(e, _)| {
            chars.next_if(|&(_, c)| matches!(c, '+' | '-'));
            let digits_at = chars.peek().map_or(source.len(), |&(i, _)| i);
            float_end = take_while(chars, source, is_digit_or_separator);
            (e, digits_at)
        });
    if fraction.is_none() && exponent.is_none() {
        return Ok(None);
    }
    if let Some(at) = fraction {
        let fraction_end = exponent.map_or(float_end, |(e, _)| e);
        check_digits(source, at, fraction_end, 10)?;
    }
    if let Some((e, digits_at)) = exponent {
        if digits_at == float_end {
            return Err(DiagnosticKind::ExponentNeedsDigits {
                at: Span::new(e, digits_at),
            }
            .into());
        }
        check_digits(source, digits_at, float_end, 10)?;
    }
    let value = source[start..float_end]
        .replace('_', "")
        .parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
        .ok_or_else(|| DiagnosticKind::FloatOutOfRange {
            at: Span::new(start, float_end),
        })?;
    Ok(Some(TokenKind::Float(Float(value))))
}

/// The `char` literal that starts at the `'` at `source[start..]`, which `chars` has consumed.
/// It reads on to the closing `'`, so that an error leaves the rest of the literal behind it, or
/// to the line end when there is none. The first error is the one reported.
fn char_literal(source: &str, start: usize, chars: &mut Chars) -> Result<TokenKind, Diagnostic> {
    let mut error = None;
    let mut points = String::new();
    let end = loop {
        let Some((i, c)) = chars.next_if(|&(_, c)| c != '\n') else {
            return Err(error.unwrap_or_else(|| {
                DiagnosticKind::LiteralNotClosed {
                    kind: LiteralKind::Char,
                    at: Span::new(start, start + 1),
                    interpolation: false,
                }
                .into()
            }));
        };
        match c {
            '\'' => break i + 1,
            // A `\` at the line end is followed by the "not closed" error of a later turn.
            '\\' if line_ends(source, chars) => {}
            '\\' => match escape(source, i, chars, LiteralKind::Char) {
                Ok(c) => points.push(c),
                Err(d) => {
                    error.get_or_insert(d);
                }
            },
            c => points.push(c),
        }
    };
    if let Some(d) = error {
        return Err(d);
    }
    let mut points = points.chars();
    match (points.next(), points.next()) {
        (Some(c), None) => Ok(TokenKind::Char(c)),
        _ => Err(DiagnosticKind::NotOneCodePoint {
            at: Span::new(start, end),
            string: !source[start..end].contains(['"', '{', '}']),
        }
        .into()),
    }
}

/// How a piece of a string literal's text ends.
pub(super) enum End {
    /// At the closing `"`.
    Quote,
    /// At the `{` of an interpolation.
    Brace,
    /// At a line break or the end of the file.
    Open,
}

/// The text of a string literal after its `"`, or after the `}` that ends an interpolation,
/// which `chars` has consumed: up to and with its closing `"` or the `{` of an interpolation, or
/// up to the line end, which the caller reports. The first error is the one reported.
pub(super) fn string_text(
    source: &str,
    chars: &mut Chars,
    diagnostics: &mut Vec<Diagnostic>,
) -> (String, End) {
    let mut error = None;
    let mut text = String::new();
    let end = loop {
        let Some((i, c)) = chars.next_if(|&(_, c)| c != '\n') else {
            break End::Open;
        };
        match c {
            '"' => break End::Quote,
            '{' => {
                let inside = &source[i + 1..];
                // What the main loop skips.
                let mut close = source.len() - inside.trim_start_matches([' ', '\t', '\r']).len();
                // A spec with no expression before it is empty too, unless it is cut off.
                if source[close..].starts_with(':') {
                    close = super::spec_end(source, close).unwrap_or(close);
                }
                if !source[close..].starts_with('}') {
                    break End::Brace;
                }
                while chars.next_if(|&(j, _)| j <= close).is_some() {}
                error.get_or_insert_with(|| {
                    DiagnosticKind::EmptyInterpolation {
                        at: Span::new(i, close + 1),
                    }
                    .into()
                });
            }
            '}' => {
                error.get_or_insert_with(|| {
                    DiagnosticKind::BraceInString {
                        at: Span::new(i, i + 1),
                    }
                    .into()
                });
            }
            // A `\` at the line end is followed by the "not closed" error of a later turn.
            '\\' if line_ends(source, chars) => {}
            '\\' => match escape(source, i, chars, LiteralKind::String) {
                Ok(c) => text.push(c),
                Err(d) => {
                    error.get_or_insert(d);
                }
            },
            c => text.push(c),
        }
    };
    diagnostics.extend(error);
    (text, end)
}

/// The error for a string literal whose `"` stands at `quote` and that a line break or the end
/// of the file stops, where an interpolation's `{` is open if `interpolation`.
pub(super) fn not_closed(quote: usize, interpolation: bool) -> Diagnostic {
    DiagnosticKind::LiteralNotClosed {
        kind: LiteralKind::String,
        at: Span::new(quote, quote + 1),
        interpolation,
    }
    .into()
}

/// Whether the line or the file ends at the next char of `chars`, where `\r\n` ends a line too.
fn line_ends(source: &str, chars: &mut Chars) -> bool {
    chars
        .peek()
        .is_none_or(|&(i, _)| source[i..].starts_with('\n') || source[i..].starts_with("\r\n"))
}

/// Decodes the escape after a `\`, which `chars` has just consumed at `backslash`. A char that is
/// not a line break must follow: the caller reports a literal that ends there as not closed.
pub(super) fn escape(
    source: &str,
    backslash: usize,
    chars: &mut Chars,
    literal: LiteralKind,
) -> Result<char, Diagnostic> {
    let Some((at, c)) = chars.next() else {
        unreachable!("the caller checked that a char follows the backslash")
    };
    match c {
        'n' => Ok('\n'),
        't' => Ok('\t'),
        'r' => Ok('\r'),
        '0' => Ok('\0'),
        '\\' | '"' | '\'' | '{' | '}' => Ok(c),
        'u' => unicode_escape(source, backslash, chars),
        _ => Err(DiagnosticKind::UnknownEscape {
            found: c,
            at: Span::new(backslash, at + c.len_utf8()),
            literal,
        }
        .into()),
    }
}

/// The value of `\u{...}`, whose `\` stands at `backslash` and whose `u` `chars` has consumed.
fn unicode_escape(source: &str, backslash: usize, chars: &mut Chars) -> Result<char, Diagnostic> {
    let malformed = |end| -> Diagnostic {
        DiagnosticKind::MalformedUnicodeEscape {
            at: Span::new(backslash, end),
        }
        .into()
    };
    let digits_at = backslash + 3;
    if chars.next_if(|&(_, c)| c == '{').is_none() {
        return Err(malformed(backslash + 2));
    }
    let digits_end = take_while(chars, source, |c| c.is_ascii_hexdigit());
    if chars.next_if(|&(_, c)| c == '}').is_none() {
        return Err(malformed(digits_end));
    }
    let end = digits_end + 1;
    let digits = &source[digits_at..digits_end];
    if digits.is_empty() || digits.len() > 6 {
        return Err(malformed(end));
    }
    u32::from_str_radix(digits, 16)
        .ok()
        .and_then(char::from_u32)
        .ok_or_else(|| {
            DiagnosticKind::NotScalarValue {
                escape: source[backslash..end].to_string(),
                at: Span::new(backslash, end),
            }
            .into()
        })
}
