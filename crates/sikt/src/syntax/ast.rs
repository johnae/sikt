use crate::class::{CharSet, case_variants};

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
            kind @ (ExprKind::Any | ExprKind::Newline | ExprKind::Anchor(_)) => kind,
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
