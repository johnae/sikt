use super::ast::{Anchor, Expr, ExprKind, Span};
use super::lexer::{Tok, Token, lex};
use crate::class::{CharSet, Kind};
use crate::error::Error;

/// Words held back for features planned after v1.
const RESERVED: &[&str] = &[
    "at", "grammar", "infix", "recover", "skip", "use", "longest",
];

/// Words that can start a pattern; used for "did you mean" suggestions.
pub(super) const VOCABULARY: &[&str] =
    &["any", "newline", "optional", "until", "through", "nocase"];

/// Words with a meaning of their own, which can't name a rule.
const KEYWORDS: &[&str] = &[
    "or", "one", "none", "of", "followed", "by", "not", "after", "line", "text", "word", "start",
    "end", "let", "find", "match",
];

/// A `let` rule as written, before its references are resolved.
pub(crate) struct Rule {
    pub name: String,
    pub span: Span,
    pub body: Expr,
}

/// A pattern as written: the expression to find and the `let` rules.
pub(crate) struct Parsed {
    pub main: Expr,
    pub rules: Vec<Rule>,
}

pub(crate) fn parse(src: &str) -> Result<Parsed, Error> {
    let tokens = lex(src)?;
    let mut p = Parser {
        src,
        tokens,
        pos: 0,
    };
    if p.peek() == &Tok::Eof {
        return Err(p.error("empty pattern", p.span(), Some("nothing to match".into())));
    }
    let mut main = if p.at_statement() {
        None
    } else {
        Some(p.expr()?)
    };
    let mut rules = Vec::new();
    loop {
        if p.is_word("let") {
            rules.push(p.rule()?);
        } else if p.is_word("find") {
            let find = p.bump();
            if main.is_some() {
                return Err(p
                    .error("the pattern to find is given twice", find.span, None)
                    .with_help("a pattern has one part to find; put the rest in `let` rules"));
            }
            if p.at_seq_end() {
                return Err(p.error("expected a pattern after `find`", find.span, None));
            }
            main = Some(p.expr()?);
        } else {
            break;
        }
    }
    match p.peek() {
        Tok::Eof => {}
        Tok::RParen => {
            return Err(p.error("unmatched `)`", p.span(), Some("no `(` before this".into())));
        }
        tok => return Err(p.error(format!("unexpected {}", tok.describe()), p.span(), None)),
    }
    match main {
        Some(main) => Ok(Parsed { main, rules }),
        None => {
            let span = Span::new(0, src.trim_end().len());
            Err(p
                .error("nothing to find", span, Some("only rules here".into()))
                .with_help("add `find` and the pattern to match after the rules"))
        }
    }
}

struct Parser<'s> {
    src: &'s str,
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> &Tok {
        &self.tokens[self.pos].tok
    }

    fn peek_second(&self) -> &Tok {
        self.tokens.get(self.pos + 1).map_or(&Tok::Eof, |t| &t.tok)
    }

    fn is_word(&self, word: &str) -> bool {
        matches!(self.peek(), Tok::Word(w) if w == word)
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn bump(&mut self) -> Token {
        let token = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        token
    }

    fn error(&self, message: impl Into<String>, span: Span, label: Option<String>) -> Error {
        Error::new(self.src, message, span, label)
    }

    /// `or` as an operator. `or:` starts a capture named `or`.
    fn at_or(&self) -> bool {
        self.is_word("or") && self.peek_second() != &Tok::Colon
    }

    /// `let` or `find` starting a statement. Followed by `:` they name a
    /// capture.
    fn at_statement(&self) -> bool {
        (self.is_word("let") || self.is_word("find")) && self.peek_second() != &Tok::Colon
    }

    fn at_seq_end(&self) -> bool {
        matches!(self.peek(), Tok::Eof | Tok::RParen) || self.at_or() || self.at_statement()
    }

    /// `let name = expr`.
    fn rule(&mut self) -> Result<Rule, Error> {
        let let_span = self.bump().span;
        let Tok::Word(name) = self.peek().clone() else {
            return Err(self.error("expected a rule name after `let`", let_span, None));
        };
        let span = self.bump().span;
        if KEYWORDS.contains(&name.as_str())
            || VOCABULARY.contains(&name.as_str())
            || RESERVED.contains(&name.as_str())
            || Kind::from_name(&name).is_some()
        {
            let message = format!("`{name}` is a built-in word and can't name a rule");
            return Err(self.error(message, span, None));
        }
        if self.peek() != &Tok::Equals {
            let message = format!("expected `=` after `let {name}`");
            return Err(self.error(message, let_span.to(span), None));
        }
        let equals = self.bump().span;
        if self.at_seq_end() {
            let message = format!("expected a pattern after `let {name} =`");
            return Err(self.error(message, let_span.to(equals), None));
        }
        let body = self.expr()?;
        Ok(Rule { name, span, body })
    }

    fn expr(&mut self) -> Result<Expr, Error> {
        if self.at_or() {
            return Err(self.error("expected a pattern before `or`", self.span(), None));
        }
        let first = self.seq()?;
        if !self.at_or() {
            return Ok(first);
        }
        let mut alts = vec![first];
        while self.at_or() {
            let or = self.bump();
            if self.at_seq_end() {
                return Err(self.error("expected a pattern after `or`", or.span, None));
            }
            alts.push(self.seq()?);
        }
        let span = alts[0].span.to(alts[alts.len() - 1].span);
        Ok(Expr::new(ExprKind::Choice(alts), span))
    }

    fn seq(&mut self) -> Result<Expr, Error> {
        let mut items = vec![self.item()?];
        while !self.at_seq_end() {
            items.push(self.item()?);
        }
        if items.len() == 1 {
            return Ok(items.pop().unwrap());
        }
        let span = items[0].span.to(items[items.len() - 1].span);
        Ok(Expr::new(ExprKind::Seq(items), span))
    }

    /// A primary, optionally preceded by prefix operators.
    fn item(&mut self) -> Result<Expr, Error> {
        let start = self.span();
        let word = match self.peek() {
            Tok::Word(w) => Some(w.clone()),
            _ => None,
        };
        let kind = match (self.peek(), word.as_deref()) {
            (Tok::Word(_), Some(name)) if self.peek_second() == &Tok::Colon => {
                if name == "match" {
                    let label = Some("reserved for the whole match".into());
                    return Err(self
                        .error("`match` can't be a capture name", start, label)
                        .with_help("pick another name, like `m`"));
                }
                let name = name.to_owned();
                self.bump();
                let colon = self.bump();
                let body = self.operand(
                    format!("expected a pattern to capture after `{name}:`"),
                    start.to(colon.span),
                )?;
                ExprKind::Capture {
                    name,
                    body: Box::new(body),
                }
            }
            (Tok::Int(_), _) => {
                let (min, max, span) = self.quantity()?;
                let text = &self.src[span.start..span.end];
                let body =
                    self.operand(format!("expected something to repeat after `{text}`"), span)?;
                ExprKind::Repeat {
                    min,
                    max,
                    body: Box::new(body),
                }
            }
            (_, Some(op @ ("optional" | "until" | "through" | "nocase"))) => {
                self.bump();
                let body =
                    Box::new(self.operand(format!("expected a pattern after `{op}`"), start)?);
                match op {
                    "optional" => ExprKind::Repeat {
                        min: 0,
                        max: Some(1),
                        body,
                    },
                    "until" => ExprKind::Until(body),
                    "through" => ExprKind::Through(body),
                    _ => ExprKind::NoCase(body),
                }
            }
            (_, Some("followed" | "not" | "after")) => self.look(start)?,
            _ => return self.prim(),
        };
        let end = self.tokens[self.pos - 1].span;
        Ok(Expr::new(kind, start.to(end)))
    }

    /// `followed by x`, `after x`, and both with `not` before them.
    fn look(&mut self, start: Span) -> Result<ExprKind, Error> {
        let negated = self.is_word("not");
        if negated {
            self.bump();
        }
        let (behind, op) = if self.is_word("after") {
            self.bump();
            (true, "after")
        } else if self.is_word("followed") {
            self.bump();
            if !self.is_word("by") {
                return Err(self.error("expected `by` after `followed`", start, None));
            }
            self.bump();
            (false, "followed by")
        } else {
            return Err(self.error("expected `followed by` or `after` after `not`", start, None));
        };
        let op_span = start.to(self.tokens[self.pos - 1].span);
        let body = self.operand(format!("expected a pattern after `{op}`"), op_span)?;
        Ok(ExprKind::Look {
            behind,
            negated,
            body: Box::new(body),
        })
    }

    /// The item after a prefix operator, or an error if there is none.
    fn operand(&mut self, missing: String, after: Span) -> Result<Expr, Error> {
        if self.at_seq_end() {
            return Err(self.error(missing, after, None));
        }
        self.item()
    }

    /// `3`, `1+` or `2..5`, as `(min, max)`.
    fn quantity(&mut self) -> Result<(u32, Option<u32>, Span), Error> {
        let Token {
            tok: Tok::Int(min),
            span,
        } = self.bump()
        else {
            unreachable!("called at a number");
        };
        match self.peek() {
            Tok::Plus => {
                let plus = self.bump();
                Ok((min, None, span.to(plus.span)))
            }
            Tok::DotDot => {
                self.bump();
                let Tok::Int(max) = *self.peek() else {
                    return Err(self.error("expected a number after `..`", self.span(), None));
                };
                let span = span.to(self.bump().span);
                if max < min {
                    let label = Some("the smaller number goes first".into());
                    return Err(self
                        .error(format!("range `{min}..{max}` is backwards"), span, label)
                        .with_help(format!("write `{max}..{min}`")));
                }
                Ok((min, Some(max), span))
            }
            _ => Ok((min, Some(min), span)),
        }
    }

    fn prim(&mut self) -> Result<Expr, Error> {
        let token = self.bump();
        let kind = match token.tok {
            Tok::Str(s) => ExprKind::Literal(s),
            Tok::LParen => {
                let inner = self.expr()?;
                if self.peek() != &Tok::RParen {
                    let label = Some("this parenthesis is never closed".into());
                    return Err(self.error("unclosed `(`", token.span, label));
                }
                let close = self.bump();
                return Ok(Expr::new(inner.kind, token.span.to(close.span)));
            }
            Tok::Word(w) => match w.as_str() {
                "any" => ExprKind::Any,
                "newline" => ExprKind::Newline,
                "one" | "none" => self.set(token.span, w == "none")?,
                "line" | "text" | "word" => return self.anchor(&w, token.span),
                w => match Kind::from_name(w) {
                    Some(kind) => ExprKind::Class(CharSet::kind(kind)),
                    None if RESERVED.contains(&w) => {
                        let message = format!("`{w}` is reserved for a future version of sikt");
                        return Err(self.error(message, token.span, None));
                    }
                    None => ExprKind::Rule(w.to_owned()),
                },
            },
            tok => {
                let help = match tok {
                    Tok::Plus => Some("repetition goes before the thing: `1+ x`"),
                    Tok::Colon => Some("captures are written `name: pattern`"),
                    _ => None,
                };
                let error = self.error(
                    format!("expected a pattern, found {}", tok.describe()),
                    token.span,
                    None,
                );
                return Err(match help {
                    Some(help) => error.with_help(help),
                    None => error,
                });
            }
        };
        Ok(Expr::new(kind, token.span))
    }

    /// `start` or `end` after `line`, `text` or `word`.
    fn anchor(&mut self, word: &str, span: Span) -> Result<Expr, Error> {
        let end = match self.peek() {
            Tok::Word(w) if w == "start" => false,
            Tok::Word(w) if w == "end" => true,
            _ => {
                let error = self.error(
                    format!("expected `start` or `end` after `{word}`"),
                    span,
                    None,
                );
                return Err(match word {
                    "word" => error.with_help("for a single word character, use `wordchar`"),
                    _ => error,
                });
            }
        };
        let last = self.bump();
        let anchor = match (word, end) {
            ("line", false) => Anchor::LineStart,
            ("line", true) => Anchor::LineEnd,
            ("text", false) => Anchor::TextStart,
            ("text", true) => Anchor::TextEnd,
            (_, false) => Anchor::WordStart,
            (_, true) => Anchor::WordEnd,
        };
        Ok(Expr::new(ExprKind::Anchor(anchor), span.to(last.span)))
    }

    /// `one of ...` or `none of ...`, after the `one` or `none`.
    fn set(&mut self, start: Span, negated: bool) -> Result<ExprKind, Error> {
        let word = if negated { "none" } else { "one" };
        if !self.is_word("of") {
            return Err(self.error(format!("expected `of` after `{word}`"), start, None));
        }
        self.bump();
        let mut ranges = Vec::new();
        let mut kinds = Vec::new();
        if self.peek() == &Tok::LParen {
            let open = self.bump();
            while !matches!(self.peek(), Tok::RParen | Tok::Eof) {
                self.set_item(&mut ranges, &mut kinds)?;
            }
            if self.peek() == &Tok::Eof {
                let label = Some("this parenthesis is never closed".into());
                return Err(self.error("unclosed `(`", open.span, label));
            }
            let close = self.bump();
            if ranges.is_empty() && kinds.is_empty() {
                let label = Some("nothing in the set".into());
                return Err(self.error("empty set", open.span.to(close.span), label));
            }
        } else {
            self.set_item(&mut ranges, &mut kinds)?;
        }
        Ok(ExprKind::Class(CharSet::new(ranges, kinds, negated)))
    }

    /// A string, a range like `"a".."f"`, or a character kind.
    fn set_item(
        &mut self,
        ranges: &mut Vec<(char, char)>,
        kinds: &mut Vec<Kind>,
    ) -> Result<(), Error> {
        let token = self.bump();
        match token.tok {
            Tok::Str(lo) if self.peek() == &Tok::DotDot => {
                self.bump();
                let hi_token = self.bump();
                let span = token.span.to(hi_token.span);
                let single = |s: &str| {
                    let mut chars = s.chars();
                    chars.next().filter(|_| chars.next().is_none())
                };
                let hi = match &hi_token.tok {
                    Tok::Str(hi) => hi.as_str(),
                    _ => "",
                };
                let (Some(lo_char), Some(hi_char)) = (single(&lo), single(hi)) else {
                    return Err(self
                        .error("a range needs single characters", span, None)
                        .with_help(r#"write it like "a".."f""#));
                };
                if hi_char < lo_char {
                    let label = Some("the smaller character goes first".into());
                    return Err(self.error(
                        format!("range {lo:?}..{hi:?} is backwards"),
                        span,
                        label,
                    ));
                }
                ranges.push((lo_char, hi_char));
            }
            Tok::Str(s) if s.is_empty() => {
                return Err(self.error("empty string in a set", token.span, None));
            }
            Tok::Str(s) => ranges.extend(s.chars().map(|c| (c, c))),
            Tok::Word(w) if Kind::from_name(&w).is_some() => kinds.extend(Kind::from_name(&w)),
            tok => {
                return Err(self
                    .error(
                        format!("{} can't be in a set", tok.describe()),
                        token.span,
                        None,
                    )
                    .with_help(r#"sets hold strings, ranges like "a".."f", and character kinds"#));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders the AST compactly, to check structure and precedence.
    fn show(e: &Expr) -> String {
        match &e.kind {
            ExprKind::Literal(s) => format!("{s:?}"),
            ExprKind::Any => "any".into(),
            ExprKind::Newline => "newline".into(),
            ExprKind::Class(set) => set.as_kind().map_or("set", |k| k.name()).into(),
            ExprKind::Seq(items) => format!(
                "seq({})",
                items.iter().map(show).collect::<Vec<_>>().join(" ")
            ),
            ExprKind::Choice(alts) => format!(
                "or({})",
                alts.iter().map(show).collect::<Vec<_>>().join(" ")
            ),
            ExprKind::Repeat { min, max, body } => match max {
                Some(max) => format!("rep{min}..{max}({})", show(body)),
                None => format!("rep{min}+({})", show(body)),
            },
            ExprKind::Capture { name, body } => format!("{name}:({})", show(body)),
            ExprKind::Anchor(a) => format!("{a:?}"),
            ExprKind::NoCase(x) => format!("nocase({})", show(x)),
            ExprKind::Rule(name) => format!("rule {name}"),
            ExprKind::Call { rule, .. } => format!("call {rule}"),
            ExprKind::Look {
                behind,
                negated,
                body,
            } => format!(
                "look{}{}({})",
                if *negated { "!" } else { "" },
                if *behind { "<" } else { ">" },
                show(body)
            ),
            ExprKind::Until(x) => format!("until({})", show(x)),
            ExprKind::Through(x) => format!("through({})", show(x)),
        }
    }

    fn ast(src: &str) -> String {
        show(&parse(src).unwrap().main)
    }

    #[test]
    fn sequence_binds_tighter_than_or() {
        assert_eq!(ast(r#""a" "b" or "c""#), r#"or(seq("a" "b") "c")"#);
    }

    #[test]
    fn prefix_operators_take_one_item() {
        assert_eq!(ast(r#"until "a" or "b""#), r#"or(until("a") "b")"#);
        assert_eq!(ast(r#"n: 1+ digit "px""#), r#"seq(n:(rep1+(digit)) "px")"#);
        assert_eq!(ast("optional 2..3 hex"), "rep0..1(rep2..3(hex))");
    }

    #[test]
    fn lookaround_takes_one_item() {
        assert_eq!(ast(r#"not followed by "a" "b""#), r#"seq(look!>("a") "b")"#);
        assert_eq!(
            ast(r#"after 2 digit "x""#),
            r#"seq(look<(rep2..2(digit)) "x")"#
        );
    }

    #[test]
    fn keywords_can_name_captures() {
        assert_eq!(
            ast("text: letter or: digit"),
            "seq(text:(letter) or:(digit))"
        );
    }

    #[test]
    fn spans_cover_operands() {
        let e = parse(r#"  n: 1+ digit"#).unwrap().main;
        assert_eq!(e.span, Span::new(2, 13));
    }
}
