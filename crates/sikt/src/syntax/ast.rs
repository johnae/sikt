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
    Until(Box<Expr>),
    Through(Box<Expr>),
}

impl ExprKind {
    /// Whether this can succeed without consuming input.
    pub fn nullable(&self) -> bool {
        match self {
            ExprKind::Literal(s) => s.is_empty(),
            ExprKind::Any | ExprKind::Newline | ExprKind::Class(_) => false,
            ExprKind::Seq(items) => items.iter().all(|e| e.kind.nullable()),
            ExprKind::Choice(alts) => alts.iter().any(|e| e.kind.nullable()),
            ExprKind::Repeat { min, body, .. } => *min == 0 || body.kind.nullable(),
            ExprKind::Capture { body, .. } => body.kind.nullable(),
            ExprKind::Until(_) => true,
            ExprKind::Through(x) => x.kind.nullable(),
        }
    }
}
