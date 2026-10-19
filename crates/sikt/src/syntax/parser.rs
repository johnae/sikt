use super::ast::{Anchor, Cond, Expr, ExprKind, Output, Span, Statements, TableSpec, Test};
use super::lexer::{Tok, Token, lex};
use crate::class::{CharSet, Kind};
use crate::error::Error;
use crate::value::CaptureType;

/// Words held back for features planned after v1.
const RESERVED: &[&str] = &[
    "at", "grammar", "infix", "recover", "skip", "use", "longest",
];

/// Words that can start a pattern; used for "did you mean" suggestions.
pub(super) const VOCABULARY: &[&str] = &[
    "any", "newline", "optional", "until", "through", "nocase", "integer", "number",
];

/// Words with a meaning of their own, which can't name a rule.
const KEYWORDS: &[&str] = &[
    "or", "one", "none", "of", "followed", "by", "not", "after", "line", "text", "word", "start",
    "end", "let", "find", "where", "replace", "print", "table", "types", "with", "match",
];

/// A `let` rule as written, before its references are resolved.
pub(crate) struct Rule {
    pub name: String,
    pub span: Span,
    pub body: Expr,
}

/// A pattern as written: the expression to find, the `let` rules, and the
/// program statements.
pub(crate) struct Parsed {
    pub main: Expr,
    pub rules: Vec<Rule>,
    pub statements: Statements,
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
    let mut statements = Statements::default();
    let mut types = Vec::new();
    let mut types_at = None;
    let mut with = Vec::new();
    loop {
        if p.is_word("let") {
            rules.push(p.rule()?);
        } else if p.is_word("table") {
            let keyword = p.bump().span;
            if statements.table.is_some() {
                return Err(p.error("a program has one `table`", keyword, None));
            }
            statements.table = Some(p.table(keyword)?);
        } else if p.is_word("types") {
            let keyword = p.bump().span;
            types_at.get_or_insert(keyword);
            types.extend(p.types(keyword)?);
        } else if p.is_word("with") {
            let keyword = p.bump().span;
            with.push(p.with(keyword)?);
        } else if p.is_word("where") {
            let keyword = p.bump().span;
            if statements.filter.is_some() {
                return Err(p
                    .error("a program has one `where`", keyword, None)
                    .with_help("combine the conditions with `and`"));
            }
            if p.at_seq_end() {
                return Err(p.error("expected a condition after `where`", keyword, None));
            }
            statements.filter = Some((keyword, p.cond()?));
        } else if p.is_word("replace") || p.is_word("print") {
            let keyword = p.bump();
            if statements.output.is_some() {
                return Err(p.error("a program has one `replace` or `print`", keyword.span, None));
            }
            let print = keyword.tok == Tok::Word("print".into());
            let Tok::Str(template) = p.peek().clone() else {
                let word = if print { "print" } else { "replace" };
                let message = format!("expected a template string after `{word}`");
                return Err(p
                    .error(message, keyword.span, None)
                    .with_help(r#"write it like "{year}-{month}""#));
            };
            let span = p.bump().span;
            statements.output = Some((
                keyword.span,
                Output {
                    print,
                    template,
                    span,
                },
            ));
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
    if let Some(table) = &mut statements.table {
        if let Some(main) = &main {
            return Err(p
                .error("a table program has no `find`", main.span, None)
                .with_help("the header says what each row holds"));
        }
        if let Some((keyword, output)) = &statements.output
            && !output.print
        {
            return Err(p.error(
                "`replace` doesn't work in a table program yet",
                *keyword,
                None,
            ));
        }
        table.with = with;
        table.types = types;
        // Rows come from the header, so the pattern itself matches nothing.
        let never = Expr::new(ExprKind::Literal(String::new()), table.span);
        let kind = ExprKind::Look {
            behind: false,
            negated: true,
            body: Box::new(never),
        };
        main = Some(Expr::new(kind, table.span));
    } else if let Some(keyword) = types_at {
        return Err(p.error("`types` only works with `table`", keyword, None));
    } else if let Some((_, keyword, _)) = with.first() {
        return Err(p.error("`with` only works with `table`", *keyword, None));
    }
    match main {
        Some(main) => Ok(Parsed {
            main,
            rules,
            statements,
        }),
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
        [
            "let", "find", "where", "replace", "print", "table", "types", "with",
        ]
        .iter()
        .any(|w| self.is_word(w))
            && self.peek_second() != &Tok::Colon
    }

    fn at_seq_end(&self) -> bool {
        matches!(self.peek(), Tok::Eof | Tok::RParen) || self.at_or() || self.at_statement()
    }

    /// A `where` condition: `or` of `and` of `not`, tests and parentheses.
    fn cond(&mut self) -> Result<Cond, Error> {
        let mut cond = self.cond_and()?;
        while self.at_or() {
            self.bump();
            cond = Cond::Or(Box::new(cond), Box::new(self.cond_and()?));
        }
        Ok(cond)
    }

    fn cond_and(&mut self) -> Result<Cond, Error> {
        let mut cond = self.cond_unary()?;
        while self.is_word("and") {
            self.bump();
            cond = Cond::And(Box::new(cond), Box::new(self.cond_unary()?));
        }
        Ok(cond)
    }

    fn cond_unary(&mut self) -> Result<Cond, Error> {
        if self.is_word("not") {
            self.bump();
            return Ok(Cond::Not(Box::new(self.cond_unary()?)));
        }
        if self.peek() == &Tok::LParen {
            let open = self.bump().span;
            let cond = self.cond()?;
            if self.peek() != &Tok::RParen {
                let label = Some("this parenthesis is never closed".into());
                return Err(self.error("unclosed `(`", open, label));
            }
            self.bump();
            return Ok(cond);
        }
        self.test()
    }

    /// `name is "x"`, `name in (...)`, `name > 5`, `name contains "x"` or
    /// `name matches (...)`, with `not` where it reads naturally.
    fn test(&mut self) -> Result<Cond, Error> {
        let Tok::Word(name) = self.peek().clone() else {
            let message = format!("expected a capture name, found {}", self.peek().describe());
            return Err(self.error(message, self.span(), None));
        };
        let span = self.bump().span;
        let mut negated = false;
        let test = if self.is_word("is") {
            self.bump();
            if self.is_word("not") {
                self.bump();
                negated = true;
            }
            Test::Is(self.string("after `is`")?)
        } else if self.is_word("in")
            || (self.is_word("not") && matches!(self.peek_second(), Tok::Word(w) if w == "in"))
        {
            negated = self.is_word("not");
            if negated {
                self.bump();
            }
            self.bump();
            Test::In(self.string_list()?)
        } else if let Tok::Cmp(op) = *self.peek() {
            let op_span = self.bump().span;
            let value = match self.peek().clone() {
                Tok::Int(n) => f64::from(n),
                Tok::Num(n) => n.parse().expect("lexer checks number syntax"),
                _ => {
                    let message = format!("expected a number after `{}`", op.symbol());
                    return Err(self.error(message, op_span, None));
                }
            };
            self.bump();
            Test::Compare(op, value)
        } else if self.is_word("contains") {
            self.bump();
            Test::Contains(self.string("after `contains`")?)
        } else if self.is_word("matches") {
            let keyword = self.bump().span;
            Test::Matches(self.operand("expected a pattern after `matches`".into(), keyword)?)
        } else {
            return Err(self
                .error(format!("expected a test after `{name}`"), span, None)
                .with_help("tests are is, is not, in, not in, > >= < <=, contains and matches"));
        };
        let capture = (name != "match").then_some(name);
        let test = Cond::Test {
            capture,
            span,
            test,
        };
        Ok(if negated {
            Cond::Not(Box::new(test))
        } else {
            test
        })
    }

    fn string(&mut self, after: &str) -> Result<String, Error> {
        match self.peek().clone() {
            Tok::Str(s) => {
                self.bump();
                Ok(s)
            }
            tok => Err(self.error(
                format!("expected a string {after}, found {}", tok.describe()),
                self.span(),
                None,
            )),
        }
    }

    /// `("a" "b" ...)`.
    fn string_list(&mut self) -> Result<Vec<String>, Error> {
        let help = r#"write the strings in parentheses, like ("WARN" "ERROR")"#;
        if self.peek() != &Tok::LParen {
            return Err(self
                .error("expected a list of strings", self.span(), None)
                .with_help(help));
        }
        let open = self.bump().span;
        let mut strings = Vec::new();
        while let Tok::Str(s) = self.peek().clone() {
            self.bump();
            strings.push(s);
        }
        if self.peek() != &Tok::RParen {
            let message = format!("expected a string or `)`, found {}", self.peek().describe());
            return Err(self
                .error(message, open.to(self.span()), None)
                .with_help(help));
        }
        self.bump();
        Ok(strings)
    }

    /// `table`, with `split by (pattern)` and `columns (names)` after it.
    fn table(&mut self, span: Span) -> Result<TableSpec, Error> {
        let mut table = TableSpec {
            span,
            split: None,
            columns: None,
            types: Vec::new(),
            with: Vec::new(),
        };
        loop {
            if self.is_word("split") {
                let split = self.bump().span;
                if !self.is_word("by") {
                    return Err(self.error("expected `by` after `split`", split, None));
                }
                let by = self.bump().span;
                table.split =
                    Some(self.operand("expected a pattern after `split by`".into(), split.to(by))?);
            } else if self.is_word("columns") {
                let columns = self.bump().span;
                let help = "name the columns in parentheses, like `columns (fs size mount)`";
                if self.peek() != &Tok::LParen {
                    return Err(self
                        .error("expected `(` after `columns`", columns, None)
                        .with_help(help));
                }
                self.bump();
                let mut names = Vec::new();
                while let Tok::Word(name) = self.peek().clone() {
                    self.bump();
                    names.push(name);
                }
                if self.peek() != &Tok::RParen {
                    let message = format!(
                        "expected a column name or `)`, found {}",
                        self.peek().describe()
                    );
                    return Err(self.error(message, self.span(), None).with_help(help));
                }
                self.bump();
                table.columns = Some(names);
            } else {
                return Ok(table);
            }
        }
    }

    /// `types name type ...`, after the `types` at `span`.
    fn types(&mut self, span: Span) -> Result<Vec<(String, Span, CaptureType)>, Error> {
        let mut types = Vec::new();
        while let Tok::Word(name) = self.peek().clone() {
            if self.at_statement() {
                break;
            }
            let name_span = self.bump().span;
            let ty = match self.peek() {
                Tok::Word(w) if w == "integer" => CaptureType::Integer,
                Tok::Word(w) if w == "number" => CaptureType::Number,
                Tok::Word(w) if w == "text" => CaptureType::Text,
                _ => {
                    let message = format!("expected a type after `{name}`");
                    return Err(self
                        .error(message, name_span, None)
                        .with_help("types are integer, number and text"));
                }
            };
            self.bump();
            types.push((name, name_span, ty));
        }
        if types.is_empty() {
            return Err(self
                .error("expected a column and its type after `types`", span, None)
                .with_help("write it like `types pid integer cpu number`"));
        }
        Ok(types)
    }

    /// `with name: pattern`, after the `with` at `span`.
    fn with(&mut self, span: Span) -> Result<(String, Span, Expr), Error> {
        let (Tok::Word(name), Tok::Colon) = (self.peek().clone(), self.peek_second().clone())
        else {
            return Err(self
                .error("expected a column and `:` after `with`", span, None)
                .with_help("write it like `with time: m: integer \":\" s: integer`"));
        };
        let name_span = self.bump().span;
        let colon = self.bump().span;
        if self.at_seq_end() {
            let message = format!("expected a pattern after `with {name}:`");
            return Err(self.error(message, span.to(colon), None));
        }
        let body = self.expr()?;
        Ok((name, name_span, body))
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
                "integer" => typed(CaptureType::Integer, token.span),
                "number" => typed(CaptureType::Number, token.span),
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
                    Tok::Equals => Some(r#"to match `=`, quote it: "=""#),
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

/// `integer` or `number` as a pattern, with every span on the word itself so
/// explanations and warnings name the word.
fn typed(ty: CaptureType, span: Span) -> ExprKind {
    let source = match ty {
        CaptureType::Integer => r#"optional one of "+-" 1+ digit"#,
        CaptureType::Number => {
            r#"optional one of "+-" (1+ digit optional ("." 1+ digit) or "." 1+ digit)"#
        }
        CaptureType::Text => unreachable!("text has no word"),
    };
    let mut body = parse(source).expect("built-in patterns parse").main;
    respan(&mut body, span);
    ExprKind::Typed {
        ty,
        body: Box::new(body),
    }
}

fn respan(e: &mut Expr, span: Span) {
    e.span = span;
    for child in e.kind.children_mut() {
        respan(child, span);
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
            ExprKind::Typed { ty, body } => format!("{ty:?}({})", show(body)),
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
