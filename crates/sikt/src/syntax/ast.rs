use crate::class::{CharSet, case_variants};
use crate::value::CaptureType;

/// A byte range in the pattern source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }

    /// The span from the start of `self` to the end of `other`.
    pub fn to(self, other: Span) -> Span {
        Span::new(self.start, other.end)
    }
}

impl From<Span> for miette::SourceSpan {
    fn from(span: Span) -> Self {
        (span.start, span.end - span.start).into()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Self {
        Expr { kind, span }
    }

    /// The same pattern, matching case-insensitively with simple case folding.
    pub fn fold_case(self) -> Expr {
        let fold = |e: Box<Expr>| Box::new(e.fold_case());
        let kind = match self.kind {
            ExprKind::Literal(s) => return fold_literal(&s, self.span),
            ExprKind::Class(set) => ExprKind::Class(set.folded()),
            ExprKind::Seq(items) => ExprKind::Seq(items.into_iter().map(Expr::fold_case).collect()),
            ExprKind::Choice(alts) => {
                ExprKind::Choice(alts.into_iter().map(Expr::fold_case).collect())
            }
            ExprKind::Repeat { min, max, body } => ExprKind::Repeat {
                min,
                max,
                body: fold(body),
            },
            ExprKind::Capture { name, body } => ExprKind::Capture {
                name,
                body: fold(body),
            },
            ExprKind::Typed { ty, body } => ExprKind::Typed {
                ty,
                body: fold(body),
            },
            ExprKind::Until(x) => ExprKind::Until(fold(x)),
            ExprKind::Through(x) => ExprKind::Through(fold(x)),
            ExprKind::Look {
                behind,
                negated,
                body,
            } => ExprKind::Look {
                behind,
                negated,
                body: fold(body),
            },
            ExprKind::Call { rule, nullable, .. } => ExprKind::Call {
                rule,
                nullable,
                nocase: true,
            },
            kind @ (ExprKind::Any | ExprKind::Newline | ExprKind::Anchor(_)) => kind,
            ExprKind::NoCase(_) | ExprKind::Rule(_) => unreachable!("resolved before folding"),
        };
        Expr::new(kind, self.span)
    }
}

/// A literal as a sequence: characters with case become sets of their case
/// variants, and runs of characters without case stay literal.
fn fold_literal(s: &str, span: Span) -> Expr {
    let mut items = Vec::new();
    let mut plain = String::new();
    for c in s.chars() {
        let variants: Vec<char> = case_variants(c).filter(|&v| v != c).collect();
        if variants.is_empty() {
            plain.push(c);
            continue;
        }
        if !plain.is_empty() {
            items.push(Expr::new(
                ExprKind::Literal(std::mem::take(&mut plain)),
                span,
            ));
        }
        let ranges = std::iter::once(c).chain(variants).map(|v| (v, v)).collect();
        items.push(Expr::new(
            ExprKind::Class(CharSet::new(ranges, Vec::new(), false).folded()),
            span,
        ));
    }
    if !plain.is_empty() || items.is_empty() {
        items.push(Expr::new(ExprKind::Literal(plain), span));
    }
    match items.len() {
        1 => items.pop().unwrap(),
        _ => Expr::new(ExprKind::Seq(items), span),
    }
}

/// A zero-width assertion about the position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Anchor {
    LineStart,
    LineEnd,
    TextStart,
    TextEnd,
    WordStart,
    WordEnd,
}

#[derive(Debug, Clone)]
pub(crate) enum ExprKind {
    Literal(String),
    Any,
    Newline,
    /// One character from a set; character kinds like `digit` are sets too.
    Class(CharSet),
    Seq(Vec<Expr>),
    /// Ordered choice: the first alternative that matches wins.
    Choice(Vec<Expr>),
    /// Possessive repetition; `max: None` is unbounded.
    Repeat {
        min: u32,
        max: Option<u32>,
        body: Box<Expr>,
    },
    Capture {
        name: String,
        body: Box<Expr>,
    },
    Anchor(Anchor),
    /// Checks `body` without consuming input: ahead from the position, or behind
    /// it for a body of fixed length. Captures inside are dropped.
    Look {
        behind: bool,
        negated: bool,
        body: Box<Expr>,
    },
    Until(Box<Expr>),
    Through(Box<Expr>),
    /// `integer` or `number`: matches `body` and gives a capture of it a typed
    /// value.
    Typed {
        ty: CaptureType,
        body: Box<Expr>,
    },
    /// `nocase x`, lowered by resolving.
    NoCase(Box<Expr>),
    /// A reference to a `let` rule by name, replaced by resolving.
    Rule(String),
    /// A call to a recursive rule, by its index in [`Ast::rules`]. Other rules
    /// are inlined.
    Call {
        rule: usize,
        nullable: bool,
        nocase: bool,
    },
}

impl ExprKind {
    /// The number of characters every match has, if it's always the same.
    pub fn fixed_len(&self) -> Option<usize> {
        match self {
            ExprKind::Literal(s) => Some(s.chars().count()),
            ExprKind::Any | ExprKind::Class(_) => Some(1),
            ExprKind::Anchor(_) | ExprKind::Look { .. } => Some(0),
            ExprKind::Newline
            | ExprKind::Until(_)
            | ExprKind::Through(_)
            | ExprKind::Call { .. } => None,
            ExprKind::Seq(items) => items.iter().map(|e| e.kind.fixed_len()).sum(),
            ExprKind::Choice(alts) => {
                let first = alts[0].kind.fixed_len()?;
                alts.iter()
                    .all(|e| e.kind.fixed_len() == Some(first))
                    .then_some(first)
            }
            ExprKind::Repeat { min, max, body } => match max {
                Some(max) if max == min => Some(body.kind.fixed_len()? * *min as usize),
                _ => None,
            },
            ExprKind::Capture { body, .. } | ExprKind::Typed { body, .. } => body.kind.fixed_len(),
            ExprKind::NoCase(_) | ExprKind::Rule(_) => unreachable!("resolved before analysis"),
        }
    }

    /// The literal every match starts with, and whether that literal is all
    /// of `self` (so a following item can extend it).
    pub fn literal_prefix(&self) -> (String, bool) {
        match self {
            ExprKind::Literal(s) => (s.clone(), true),
            ExprKind::Anchor(_) | ExprKind::Look { .. } => (String::new(), true),
            ExprKind::Seq(items) => {
                let mut prefix = String::new();
                for item in items {
                    let (p, complete) = item.kind.literal_prefix();
                    prefix.push_str(&p);
                    if !complete {
                        return (prefix, false);
                    }
                }
                (prefix, true)
            }
            ExprKind::Capture { body, .. } | ExprKind::Typed { body, .. } => {
                body.kind.literal_prefix()
            }
            ExprKind::Repeat { min, body, .. } if *min >= 1 => {
                (body.kind.literal_prefix().0, false)
            }
            _ => (String::new(), false),
        }
    }

    /// Whether this can succeed without consuming input.
    pub fn nullable(&self) -> bool {
        match self {
            ExprKind::Literal(s) => s.is_empty(),
            ExprKind::Any | ExprKind::Newline | ExprKind::Class(_) => false,
            ExprKind::Seq(items) => items.iter().all(|e| e.kind.nullable()),
            ExprKind::Choice(alts) => alts.iter().any(|e| e.kind.nullable()),
            ExprKind::Repeat { min, body, .. } => *min == 0 || body.kind.nullable(),
            ExprKind::Capture { body, .. } | ExprKind::Typed { body, .. } => body.kind.nullable(),
            ExprKind::Until(_) | ExprKind::Anchor(_) | ExprKind::Look { .. } => true,
            ExprKind::Through(x) => x.kind.nullable(),
            ExprKind::Call { nullable, .. } => *nullable,
            ExprKind::NoCase(_) | ExprKind::Rule(_) => unreachable!("resolved before analysis"),
        }
    }

    pub fn children(&self) -> &[Expr] {
        match self {
            ExprKind::Seq(items) | ExprKind::Choice(items) => items,
            ExprKind::Repeat { body, .. }
            | ExprKind::Capture { body, .. }
            | ExprKind::Typed { body, .. }
            | ExprKind::Look { body, .. }
            | ExprKind::Until(body)
            | ExprKind::Through(body)
            | ExprKind::NoCase(body) => std::slice::from_ref(body),
            ExprKind::Literal(_)
            | ExprKind::Any
            | ExprKind::Newline
            | ExprKind::Class(_)
            | ExprKind::Anchor(_)
            | ExprKind::Rule(_)
            | ExprKind::Call { .. } => &[],
        }
    }

    pub fn children_mut(&mut self) -> &mut [Expr] {
        match self {
            ExprKind::Seq(items) | ExprKind::Choice(items) => items,
            ExprKind::Repeat { body, .. }
            | ExprKind::Capture { body, .. }
            | ExprKind::Typed { body, .. }
            | ExprKind::Look { body, .. }
            | ExprKind::Until(body)
            | ExprKind::Through(body)
            | ExprKind::NoCase(body) => std::slice::from_mut(body),
            ExprKind::Literal(_)
            | ExprKind::Any
            | ExprKind::Newline
            | ExprKind::Class(_)
            | ExprKind::Anchor(_)
            | ExprKind::Rule(_)
            | ExprKind::Call { .. } => &mut [],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CmpOp {
    Gt,
    Ge,
    Lt,
    Le,
}

impl CmpOp {
    pub fn symbol(self) -> &'static str {
        match self {
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
        }
    }

    pub fn holds(self, a: f64, b: f64) -> bool {
        match self {
            CmpOp::Gt => a > b,
            CmpOp::Ge => a >= b,
            CmpOp::Lt => a < b,
            CmpOp::Le => a <= b,
        }
    }
}

/// A `where` condition.
#[derive(Debug, Clone)]
pub(crate) enum Cond {
    And(Box<Cond>, Box<Cond>),
    Or(Box<Cond>, Box<Cond>),
    Not(Box<Cond>),
    /// A test on a capture, or on the whole match when `capture` is `None`.
    Test {
        capture: Option<String>,
        span: Span,
        test: Test,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum Test {
    Is(String),
    In(Vec<String>),
    Compare(CmpOp, f64),
    Contains(String),
    /// The sub-pattern matches all of the text.
    Matches(Expr),
}

impl Cond {
    /// Applies `f` to every sub-pattern in the condition.
    pub fn try_map_exprs<E>(self, f: &mut impl FnMut(Expr) -> Result<Expr, E>) -> Result<Cond, E> {
        Ok(match self {
            Cond::And(a, b) => {
                Cond::And(Box::new(a.try_map_exprs(f)?), Box::new(b.try_map_exprs(f)?))
            }
            Cond::Or(a, b) => {
                Cond::Or(Box::new(a.try_map_exprs(f)?), Box::new(b.try_map_exprs(f)?))
            }
            Cond::Not(c) => Cond::Not(Box::new(c.try_map_exprs(f)?)),
            Cond::Test {
                capture,
                span,
                test: Test::Matches(e),
            } => Cond::Test {
                capture,
                span,
                test: Test::Matches(f(e)?),
            },
            test @ Cond::Test { .. } => test,
        })
    }
}

/// `replace "..."` or `print "..."`.
#[derive(Debug, Clone)]
pub(crate) struct Output {
    pub print: bool,
    /// The template text, from inside the string.
    pub template: String,
    /// The string, quotes included.
    pub span: Span,
}

/// The parts that make a pattern a program, with the span of the keyword
/// that starts each.
#[derive(Debug, Clone, Default)]
pub(crate) struct Statements {
    pub filter: Option<(Span, Cond)>,
    pub output: Option<(Span, Output)>,
}

/// A resolved pattern: the main expression and the recursive rules it calls.
#[derive(Debug, Clone)]
pub(crate) struct Ast {
    pub main: Expr,
    pub rules: Vec<Expr>,
    /// Rules the main expression never reaches, with the span of their name.
    pub unused_rules: Vec<(String, Span)>,
    pub statements: Statements,
}
