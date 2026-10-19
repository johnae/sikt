use std::borrow::Cow;

use crate::compile::{self, compile};
use crate::error::{Error, MatchError, Warning};
use crate::haystack::Haystack;
use crate::pattern::{Match, Pattern, PatternBuilder, splice};
use crate::syntax::{
    Anchor, Ast, CmpOp, Cond, Expr, ExprKind, Span, Statements, Test, suggest_from,
};
use crate::table::Table;
use crate::template::Template;
use crate::value::Value;
use crate::vm::{self, Scratch};

/// What a program does with each match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Only find: no `replace` or `print`.
    Find,
    Replace,
    Print,
}

/// A pattern with the statements around it: `where` to filter matches, and
/// `replace` or `print` to say what to do with them.
///
/// ```
/// let p = sikt::Program::new(r#"find n: 1+ digit where n > 9 replace "<{n}>""#)?;
/// assert_eq!(p.replace_all("1 12 123")?, "1 <12> <123>");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct Program {
    pattern: Pattern,
    filter: Option<Filter>,
    output: Option<Template>,
    action: Action,
    case_insensitive: bool,
    table: Option<Table>,
}

#[derive(Debug)]
enum Filter {
    And(Box<Filter>, Box<Filter>),
    Or(Box<Filter>, Box<Filter>),
    Not(Box<Filter>),
    Test {
        capture: Option<String>,
        test: CheckedTest,
    },
}

#[derive(Debug)]
enum CheckedTest {
    Is(String),
    In(Vec<String>),
    Compare(CmpOp, f64),
    Contains(String),
    Matches(Box<compile::Code>),
}

impl Program {
    pub fn new(source: &str) -> Result<Program, Error> {
        Pattern::builder().build_program(source)
    }

    pub(crate) fn from_parts(
        pattern: Pattern,
        statements: Statements,
        ast: &Ast,
        source: &str,
        builder: &PatternBuilder,
    ) -> Result<Program, Error> {
        let case_insensitive = builder.case_insensitive;
        if let Some(spec) = statements.table {
            // Checking `where` and the template waits for a header to name the
            // columns; see [`Table::for_header`].
            let action = match &statements.output {
                Some((_, output)) if output.print => Action::Print,
                _ => Action::Find,
            };
            let statements = Statements {
                table: None,
                ..statements
            };
            let table = Table::new(spec, statements, ast, source, builder);
            return Ok(Program {
                pattern,
                filter: None,
                output: None,
                action,
                case_insensitive,
                table: Some(table),
            });
        }
        let names: Vec<&str> = pattern.capture_names().collect();
        let filter = match statements.filter {
            Some((_, cond)) => Some(check(cond, ast, &names, source, case_insensitive)?),
            None => None,
        };
        let (action, output) = match statements.output {
            None => (Action::Find, None),
            Some((_, output)) => {
                let template = Template::parse(&output.template, &names)
                    .map_err(|p| p.into_error(source, output.span.start + 1))?;
                let action = if output.print {
                    Action::Print
                } else {
                    Action::Replace
                };
                (action, Some(template))
            }
        };
        Ok(Program {
            pattern,
            filter,
            output,
            action,
            case_insensitive,
            table: None,
        })
    }

    /// For a `table` program, the table, which compiles a program for the rows
    /// under each header. The table program itself matches nothing.
    pub fn table(&self) -> Option<&Table> {
        self.table.as_ref()
    }

    pub fn pattern(&self) -> &Pattern {
        &self.pattern
    }

    pub fn action(&self) -> Action {
        self.action
    }

    pub fn warnings(&self) -> &[Warning] {
        self.pattern.warnings()
    }

    /// The matches that pass `where`, left to right. A match that `where`
    /// rejects still consumes its text, so the scan resumes after it.
    pub fn find_iter<'a>(
        &'a self,
        text: &'a str,
    ) -> impl Iterator<Item = Result<Match<'a>, MatchError>> + 'a {
        self.search(text)
    }

    /// Like [`find_iter`](Self::find_iter), on bytes that may not be valid
    /// UTF-8.
    pub fn find_iter_bytes<'a>(
        &'a self,
        text: &'a [u8],
    ) -> impl Iterator<Item = Result<Match<'a, [u8]>, MatchError>> + 'a {
        self.search(text)
    }

    fn search<'a, H: Haystack + ?Sized>(
        &'a self,
        text: &'a H,
    ) -> impl Iterator<Item = Result<Match<'a, H>, MatchError>> + 'a {
        self.pattern.search(text).filter_map(move |m| {
            m.and_then(|m| Ok(self.accepts(&m)?.then_some(m)))
                .transpose()
        })
    }

    /// What `replace` or `print` makes of `m`; `None` for a program that only
    /// finds.
    pub fn render(&self, m: &Match) -> Option<String> {
        self.output.as_ref().map(|t| t.render(m))
    }

    /// The text with every accepted match replaced. A program without
    /// `replace` leaves the text as it is.
    pub fn replace_all<'a>(&self, text: &'a str) -> Result<Cow<'a, str>, MatchError> {
        match (&self.output, self.action) {
            (Some(template), Action::Replace) => splice(text, self.find_iter(text), |m, out| {
                template.render_into(m, out)
            }),
            _ => Ok(Cow::Borrowed(text)),
        }
    }

    fn accepts<H: Haystack + ?Sized>(&self, m: &Match<'_, H>) -> Result<bool, MatchError> {
        match &self.filter {
            Some(filter) => self.eval(filter, m),
            None => Ok(true),
        }
    }

    fn eval<H: Haystack + ?Sized>(
        &self,
        filter: &Filter,
        m: &Match<'_, H>,
    ) -> Result<bool, MatchError> {
        Ok(match filter {
            Filter::And(a, b) => self.eval(a, m)? && self.eval(b, m)?,
            Filter::Or(a, b) => self.eval(a, m)? || self.eval(b, m)?,
            Filter::Not(f) => !self.eval(f, m)?,
            Filter::Test { capture, test } => {
                // A capture that didn't participate tests as empty text.
                let text = match capture {
                    Some(name) => m.get(name).map_or(&[][..], Haystack::bytes),
                    None => m.text().bytes(),
                };
                let folded = || -> Cow<'_, [u8]> {
                    if self.case_insensitive {
                        Cow::Owned(String::from_utf8_lossy(text).to_lowercase().into_bytes())
                    } else {
                        Cow::Borrowed(text)
                    }
                };
                match test {
                    CheckedTest::Is(s) => *folded() == *s.as_bytes(),
                    CheckedTest::In(list) => {
                        let folded = folded();
                        list.iter().any(|s| *s.as_bytes() == *folded)
                    }
                    CheckedTest::Compare(op, n) => {
                        // A typed capture compares by its value, so `16G` is bytes;
                        // other text compares if it reads as a number.
                        let value = match capture.as_deref().and_then(|name| m.value(name)) {
                            Some(Value::Integer(i)) => Some(i as f64),
                            Some(Value::Number(x)) => Some(x),
                            Some(Value::Text(_)) | None => {
                                std::str::from_utf8(text).ok().and_then(|t| t.parse().ok())
                            }
                        };
                        value.is_some_and(|v| op.holds(v, *n))
                    }
                    CheckedTest::Contains(s) => {
                        memchr::memmem::find(&folded(), s.as_bytes()).is_some()
                    }
                    CheckedTest::Matches(program) => {
                        let mut scratch = Scratch::new(program);
                        vm::exec(program, text, 0, &mut scratch, self.pattern.limits())?.is_some()
                    }
                }
            }
        })
    }
}

/// Checks capture names and compiles sub-patterns. In a case-insensitive
/// program, strings are compared in lowercase.
fn check(
    cond: Cond,
    ast: &Ast,
    names: &[&str],
    source: &str,
    case_insensitive: bool,
) -> Result<Filter, Error> {
    let check = |c: Cond| check(c, ast, names, source, case_insensitive);
    let fold = |s: String| {
        if case_insensitive {
            s.to_lowercase()
        } else {
            s
        }
    };
    Ok(match cond {
        Cond::And(a, b) => Filter::And(Box::new(check(*a)?), Box::new(check(*b)?)),
        Cond::Or(a, b) => Filter::Or(Box::new(check(*a)?), Box::new(check(*b)?)),
        Cond::Not(c) => Filter::Not(Box::new(check(*c)?)),
        Cond::Test {
            capture,
            span,
            test,
        } => {
            if let Some(name) = &capture
                && !names.contains(&name.as_str())
            {
                return Err(unknown_capture(source, name, span, names));
            }
            let test = match test {
                Test::Is(s) => CheckedTest::Is(fold(s)),
                Test::In(list) => CheckedTest::In(list.into_iter().map(fold).collect()),
                Test::Compare(op, n) => CheckedTest::Compare(op, n),
                Test::Contains(s) => CheckedTest::Contains(fold(s)),
                Test::Matches(expr) => {
                    CheckedTest::Matches(Box::new(whole_text(expr, ast, source, case_insensitive)?))
                }
            };
            Filter::Test { capture, test }
        }
    })
}

/// Compiles `expr` to match all of a text, with the program's rules.
fn whole_text(
    expr: Expr,
    ast: &Ast,
    source: &str,
    case_insensitive: bool,
) -> Result<compile::Code, Error> {
    let expr = if case_insensitive {
        expr.fold_case()
    } else {
        expr
    };
    let span = expr.span;
    let anchor = |a| Expr::new(ExprKind::Anchor(a), span);
    let main = Expr::new(
        ExprKind::Seq(vec![
            anchor(Anchor::TextStart),
            expr,
            anchor(Anchor::TextEnd),
        ]),
        span,
    );
    let sub = Ast {
        main,
        rules: ast.rules.clone(),
        unused_rules: Vec::new(),
        statements: Statements::default(),
    };
    compile(&sub, source)
}

fn unknown_capture(source: &str, name: &str, span: Span, names: &[&str]) -> Error {
    let candidates = names.iter().copied().chain(["match"]);
    let label = suggest_from(name, candidates).map(|s| format!("did you mean `{s}`?"));
    let help = if names.is_empty() {
        "this pattern has no captures; `match` is the whole match".to_owned()
    } else {
        format!("captures in this pattern: {}", names.join(", "))
    };
    Error::new(source, format!("unknown capture `{name}`"), span, label).with_help(help)
}
