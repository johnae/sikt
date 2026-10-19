use std::fmt;
use std::ops::Range;

use miette::{Diagnostic, LabeledSpan, NamedSource, Severity, SourceCode};

use crate::syntax::Span;

/// A pattern that failed to compile, with the location of the problem.
///
/// Implements [`miette::Diagnostic`], so it renders with the source line and
/// a label pointing at the problem.
pub struct Error(Box<Inner>);

struct Inner {
    message: String,
    source_code: NamedSource<String>,
    label: LabeledSpan,
    help: Option<String>,
}

impl Error {
    pub(crate) fn new(
        src: &str,
        message: impl Into<String>,
        span: Span,
        label: Option<String>,
    ) -> Self {
        Error(Box::new(Inner {
            message: message.into(),
            source_code: NamedSource::new("pattern", src.to_owned()),
            label: LabeledSpan::new_primary_with_span(label, span),
            help: None,
        }))
    }

    pub(crate) fn with_help(mut self, help: impl Into<String>) -> Self {
        self.0.help = Some(help.into());
        self
    }

    pub fn message(&self) -> &str {
        &self.0.message
    }

    /// Byte range in the pattern source the error points at.
    pub fn span(&self) -> Range<usize> {
        let span = self.0.label.inner();
        span.offset()..span.offset() + span.len()
    }

    /// Text attached to the span, like ``did you mean `digit`?``.
    pub fn label(&self) -> Option<&str> {
        self.0.label.label()
    }

    pub fn help(&self) -> Option<&str> {
        self.0.help.as_deref()
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Error")
            .field("message", &self.0.message)
            .field("span", &self.span())
            .field("label", &self.label())
            .field("help", &self.0.help)
            .finish()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.message)
    }
}

impl std::error::Error for Error {}

impl Diagnostic for Error {
    fn source_code(&self) -> Option<&dyn SourceCode> {
        Some(&self.0.source_code)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        Some(Box::new(std::iter::once(self.0.label.clone())))
    }

    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        self.0
            .help
            .as_ref()
            .map(|h| Box::new(h) as Box<dyn fmt::Display>)
    }
}

/// An error while matching, as opposed to a pattern that doesn't match.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MatchError {
    /// One attempt to match ran more VM steps than the pattern allows.
    #[error("pattern exceeded its step budget of {limit} steps")]
    StepLimit { limit: u64 },
    /// Rules called each other more deeply than the pattern allows.
    #[error("pattern exceeded its call depth of {limit}")]
    DepthLimit { limit: u32 },
}

/// Something in a pattern that compiles but probably doesn't do what was
/// meant, like an option that can never match.
///
/// Implements [`miette::Diagnostic`] with warning severity.
pub struct Warning(Error);

impl Warning {
    pub(crate) fn new(
        src: &str,
        message: impl Into<String>,
        span: Span,
        help: impl Into<String>,
    ) -> Self {
        Warning(Error::new(src, message, span, None).with_help(help))
    }

    pub fn message(&self) -> &str {
        self.0.message()
    }

    /// Byte range in the pattern source the warning points at.
    pub fn span(&self) -> Range<usize> {
        self.0.span()
    }

    pub fn help(&self) -> Option<&str> {
        self.0.help()
    }
}

impl fmt::Debug for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Warning")
            .field("message", &self.message())
            .field("span", &self.span())
            .field("help", &self.help())
            .finish()
    }
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for Warning {}

impl Diagnostic for Warning {
    fn severity(&self) -> Option<Severity> {
        Some(Severity::Warning)
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        self.0.source_code()
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        self.0.labels()
    }

    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Diagnostic::help(&self.0)
    }
}
