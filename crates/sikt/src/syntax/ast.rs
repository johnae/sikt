use crate::class::CharSet;

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
}

impl ExprKind {
    /// The number of characters every match has, if it's always the same.
    pub fn fixed_len(&self) -> Option<usize> {
        match self {
            ExprKind::Literal(s) => Some(s.chars().count()),
            ExprKind::Any | ExprKind::Class(_) => Some(1),
            ExprKind::Anchor(_) | ExprKind::Look { .. } => Some(0),
            ExprKind::Newline | ExprKind::Until(_) | ExprKind::Through(_) => None,
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
            ExprKind::Capture { body, .. } => body.kind.fixed_len(),
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
            ExprKind::Capture { body, .. } => body.kind.nullable(),
            ExprKind::Until(_) | ExprKind::Anchor(_) | ExprKind::Look { .. } => true,
            ExprKind::Through(x) => x.kind.nullable(),
        }
    }
}
