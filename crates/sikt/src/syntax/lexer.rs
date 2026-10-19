use std::iter::Peekable;
use std::str::CharIndices;

use super::ast::Span;
use crate::error::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Tok {
    Str(String),
    Word(String),
    Int(u32),
    Plus,
    DotDot,
    Colon,
    Equals,
    LParen,
    RParen,
    Eof,
}

impl Tok {
    /// How the token is named in error messages.
    pub fn describe(&self) -> String {
        match self {
            Tok::Str(s) => format!("{s:?}"),
            Tok::Word(w) => format!("`{w}`"),
            Tok::Int(n) => format!("`{n}`"),
            Tok::Plus => "`+`".into(),
            Tok::DotDot => "`..`".into(),
            Tok::Colon => "`:`".into(),
            Tok::Equals => "`=`".into(),
            Tok::LParen => "`(`".into(),
            Tok::RParen => "`)`".into(),
            Tok::Eof => "the end of the pattern".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub tok: Tok,
    pub span: Span,
}

type Chars<'a> = Peekable<CharIndices<'a>>;

/// Splits a pattern into tokens. The last token is always [`Tok::Eof`].
pub(crate) fn lex(src: &str) -> Result<Vec<Token>, Error> {
    let mut tokens = Vec::new();
    let mut chars = src.char_indices().peekable();
    while let Some(&(start, c)) = chars.peek() {
        let tok = match c {
            c if c.is_whitespace() => {
                chars.next();
                continue;
            }
            '#' => {
                while chars.next_if(|&(_, c)| c != '\n').is_some() {}
                continue;
            }
            '"' | '\'' => Tok::Str(string(src, &mut chars)?),
            '(' | ')' | ':' | '=' | '+' => {
                chars.next();
                match c {
                    '(' => Tok::LParen,
                    ')' => Tok::RParen,
                    ':' => Tok::Colon,
                    '=' => Tok::Equals,
                    _ => Tok::Plus,
                }
            }
            '.' => {
                chars.next();
                if chars.next_if(|&(_, c)| c == '.').is_none() {
                    return Err(unexpected(src, '.', Span::new(start, start + 1)));
                }
                Tok::DotDot
            }
            '0'..='9' => {
                while chars.next_if(|(_, c)| c.is_ascii_digit()).is_some() {}
                let span = Span::new(start, offset(src, &mut chars));
                match src[span.start..span.end].parse() {
                    Ok(n) => Tok::Int(n),
                    Err(_) => return Err(Error::new(src, "number is too large", span, None)),
                }
            }
            c if c.is_alphabetic() || c == '_' => {
                while chars
                    .next_if(|&(_, c)| c.is_alphanumeric() || c == '_')
                    .is_some()
                {}
                Tok::Word(src[start..offset(src, &mut chars)].to_owned())
            }
            c => return Err(unexpected(src, c, Span::new(start, start + c.len_utf8()))),
        };
        let span = Span::new(start, offset(src, &mut chars));
        tokens.push(Token { tok, span });
    }
    let end = Span::new(src.len(), src.len());
    tokens.push(Token {
        tok: Tok::Eof,
        span: end,
    });
    Ok(tokens)
}

fn offset(src: &str, chars: &mut Chars) -> usize {
    chars.peek().map_or(src.len(), |&(i, _)| i)
}

/// Lexes a quoted string, starting at its opening quote.
fn string(src: &str, chars: &mut Chars) -> Result<String, Error> {
    let (start, quote) = chars.next().expect("called at a quote");
    let mut out = String::new();
    loop {
        let Some((i, c)) = chars.next() else {
            let span = Span::new(start, src.len());
            return Err(Error::new(
                src,
                "unterminated string",
                span,
                Some("this string never ends".into()),
            )
            .with_help(format!("add a closing {quote}")));
        };
        match c {
            c if c == quote => return Ok(out),
            '\\' => out.push(escape(src, i, chars)?),
            c => out.push(c),
        }
    }
}

/// Lexes the escape after a backslash at `start`.
fn escape(src: &str, start: usize, chars: &mut Chars) -> Result<char, Error> {
    let bad = |chars: &mut Chars, message: String| {
        let span = Span::new(start, offset(src, chars));
        Error::new(src, message, span, None)
            .with_help(r#"valid escapes are \n \t \r \\ \" \' and \u{1F600}"#)
    };
    let Some((_, c)) = chars.next() else {
        return Err(bad(chars, "unterminated escape".into()));
    };
    Ok(match c {
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        '\\' | '"' | '\'' => c,
        'u' => {
            if chars.next_if(|&(_, c)| c == '{').is_none() {
                return Err(bad(chars, r"expected `{` after `\u`".into()));
            }
            let mut hex = String::new();
            while let Some((_, c)) = chars.next_if(|(_, c)| c.is_ascii_hexdigit()) {
                hex.push(c);
            }
            let closed = chars.next_if(|&(_, c)| c == '}').is_some();
            let code = u32::from_str_radix(&hex, 16)
                .ok()
                .filter(|_| closed && hex.len() <= 6);
            match code.and_then(char::from_u32) {
                Some(c) => c,
                None => return Err(bad(chars, "invalid unicode escape".into())),
            }
        }
        c => return Err(bad(chars, format!(r"unknown escape `\{c}`"))),
    })
}

/// An unexpected character, with a hint when it looks like a regex habit.
fn unexpected(src: &str, c: char, span: Span) -> Error {
    let help = match c {
        '*' => "repetition goes before the thing: `0+ x` or `1+ x`".into(),
        '?' => "use `optional x`".into(),
        '|' => "use `or` between alternatives".into(),
        '.' => r#"to match a dot, quote it: ".""#.into(),
        '\\' => r#"escapes only work inside quotes, like "\n""#.into(),
        '^' | '$' => "use `line start` / `line end`".into(),
        c => format!("to match it literally, quote it: \"{c}\""),
    };
    Error::new(src, format!("unexpected `{c}`"), span, None).with_help(help)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        lex(src).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn quantities() {
        assert_eq!(
            toks("1+ 2..5"),
            [
                Tok::Int(1),
                Tok::Plus,
                Tok::Int(2),
                Tok::DotDot,
                Tok::Int(5),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn strings_with_escapes() {
        assert_eq!(
            toks(r#""a\n\"" 'b\'' "\u{1F600}""#),
            [
                Tok::Str("a\n\"".into()),
                Tok::Str("b'".into()),
                Tok::Str("😀".into()),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn comments_and_captures() {
        assert_eq!(
            toks("year: digit # four of them\n'x'"),
            [
                Tok::Word("year".into()),
                Tok::Colon,
                Tok::Word("digit".into()),
                Tok::Str("x".into()),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn spans_are_byte_ranges() {
        let tokens = lex("é: 'ü'").unwrap();
        assert_eq!(tokens[0].span, Span::new(0, 2));
        assert_eq!(tokens[2].span, Span::new(4, 8));
    }
}
