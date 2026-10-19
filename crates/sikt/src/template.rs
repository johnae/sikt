//! Replacement templates like `{day}/{month}/{year}`.

use std::ops::Range;

use crate::error::Error;
use crate::pattern::{Match, Pattern};
use crate::syntax::{Span, suggest_from};

const FILTERS: &[&str] = &["lower", "upper", "trim", "len", "default"];

/// A replacement template, checked against a pattern's captures.
///
/// `{name}` is a capture's text and `{match}` the whole match. Filters follow
/// a `|`: `lower`, `upper`, `trim`, `len`, and `default 'text'` for a capture
/// that didn't participate. `{{` and `}}` are literal braces.
#[derive(Debug, Clone)]
pub struct Template {
    parts: Vec<Part>,
}

#[derive(Debug, Clone)]
enum Part {
    Text(String),
    /// A capture, or the whole match when `name` is `None`.
    Capture {
        name: Option<String>,
        filters: Vec<Filter>,
    },
}

#[derive(Debug, Clone)]
enum Filter {
    Lower,
    Upper,
    Trim,
    Len,
    Default(String),
}

/// A template error, with its span relative to the template text.
pub(crate) struct Problem {
    message: String,
    span: Span,
    label: Option<String>,
    help: String,
}

impl Problem {
    /// The error in `src`, where the template text starts at byte `offset`.
    pub(crate) fn into_error(self, src: &str, offset: usize) -> Error {
        let span = Span::new(self.span.start + offset, self.span.end + offset);
        Error::new(src, self.message, span, self.label).with_help(self.help)
    }
}

impl Template {
    pub fn new(source: &str, pattern: &Pattern) -> Result<Template, Error> {
        let names: Vec<&str> = pattern.capture_names().collect();
        Template::parse(source, &names).map_err(|p| p.into_error(source, 0))
    }

    pub(crate) fn parse(source: &str, names: &[&str]) -> Result<Template, Problem> {
        let mut parts = Vec::new();
        let mut text = String::new();
        let mut chars = source.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            match c {
                '{' if chars.next_if(|&(_, c)| c == '{').is_some() => text.push('{'),
                '}' if chars.next_if(|&(_, c)| c == '}').is_some() => text.push('}'),
                '}' => {
                    let help = "write `}}` for a literal brace";
                    return Err(problem("unmatched `}`", i..i + 1, None, help));
                }
                '{' => {
                    let Some(close) = closing_brace(&source[i..]).map(|n| i + n) else {
                        let help = "close it with `}`, or write `{{` for a literal brace";
                        return Err(problem("unclosed `{`", i..i + 1, None, help));
                    };
                    while chars.next_if(|&(j, _)| j <= close).is_some() {}
                    if !text.is_empty() {
                        parts.push(Part::Text(std::mem::take(&mut text)));
                    }
                    parts.push(placeholder(&source[i + 1..close], i + 1, names)?);
                }
                c => text.push(c),
            }
        }
        if !text.is_empty() {
            parts.push(Part::Text(text));
        }
        Ok(Template { parts })
    }

    /// The text that replaces `m`.
    pub fn render(&self, m: &Match) -> String {
        let mut out = String::new();
        self.render_into(m, &mut out);
        out
    }

    pub(crate) fn render_into(&self, m: &Match, out: &mut String) {
        for part in &self.parts {
            match part {
                Part::Text(text) => out.push_str(text),
                Part::Capture { name, filters } => {
                    let mut value = match name {
                        None => Some(m.as_str().to_owned()),
                        Some(name) => m.get(name).map(str::to_owned),
                    };
                    for filter in filters {
                        value = match (filter, value) {
                            (Filter::Default(text), None) => Some(text.clone()),
                            (_, None) => None,
                            (Filter::Default(_), value) => value,
                            (Filter::Lower, Some(v)) => Some(v.to_lowercase()),
                            (Filter::Upper, Some(v)) => Some(v.to_uppercase()),
                            (Filter::Trim, Some(v)) => Some(v.trim().to_owned()),
                            (Filter::Len, Some(v)) => Some(v.chars().count().to_string()),
                        };
                    }
                    out.push_str(value.as_deref().unwrap_or(""));
                }
            }
        }
    }
}

fn problem(
    message: impl Into<String>,
    span: Range<usize>,
    label: Option<String>,
    help: impl Into<String>,
) -> Problem {
    Problem {
        message: message.into(),
        span: Span::new(span.start, span.end),
        label,
        help: help.into(),
    }
}

/// Offset of the `}` that closes the `{` at the start of `s`, skipping braces
/// inside quoted strings.
fn closing_brace(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in s.char_indices().skip(1) {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '}') => return Some(i),
            _ => {}
        }
    }
    None
}

/// The inside of `{...}`, which starts at byte `offset` of the template.
fn placeholder(inner: &str, offset: usize, names: &[&str]) -> Result<Part, Problem> {
    let mut segments = split_pipes(inner).into_iter().map(|(start, segment)| {
        let leading = segment.len() - segment.trim_start().len();
        (offset + start + leading, segment.trim())
    });
    let (name_start, name) = segments.next().expect("split yields at least one segment");
    if name.is_empty() {
        let help = "write a capture name like `{year}`, or `{{` for a literal brace";
        let span = offset - 1..offset + inner.len() + 1;
        return Err(problem("empty `{}`", span, None, help));
    }
    let name = if name == "match" {
        None
    } else if names.contains(&name) {
        Some(name.to_owned())
    } else {
        let candidates = names.iter().copied().chain(["match"]);
        let label = suggest_from(name, candidates).map(|s| format!("did you mean `{s}`?"));
        let help = if names.is_empty() {
            "this pattern has no captures; `{match}` is the whole match".to_owned()
        } else {
            format!("captures in this pattern: {}", names.join(", "))
        };
        let span = name_start..name_start + name.len();
        return Err(problem(
            format!("unknown capture `{name}`"),
            span,
            label,
            help,
        ));
    };

    let mut filters = Vec::new();
    for (start, text) in segments {
        let (word, arg) = text
            .split_once(char::is_whitespace)
            .map_or((text, ""), |(w, a)| (w, a.trim()));
        filters.push(match word {
            "lower" => Filter::Lower,
            "upper" => Filter::Upper,
            "trim" => Filter::Trim,
            "len" => Filter::Len,
            "default" => match unquote(arg) {
                Some(value) => Filter::Default(value),
                None => {
                    let span = start..start + text.len();
                    let help = "write it like `default 'n/a'`";
                    return Err(problem("`default` needs a string", span, None, help));
                }
            },
            word => {
                let label = suggest_from(word, FILTERS.iter().copied())
                    .map(|s| format!("did you mean `{s}`?"));
                let help = "filters are lower, upper, trim, len and default";
                let span = start..start + word.len();
                return Err(problem(
                    format!("unknown filter `{word}`"),
                    span,
                    label,
                    help,
                ));
            }
        });
    }
    Ok(Part::Capture { name, filters })
}

/// Splits on `|` outside quotes, keeping each segment's offset.
fn split_pipes(s: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut quote = None;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '|') => {
                out.push((start, &s[start..i]));
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push((start, &s[start..]));
    out
}

/// The contents of a string in single or double quotes.
fn unquote(s: &str) -> Option<String> {
    let quote = s.chars().next().filter(|&c| c == '"' || c == '\'')?;
    let inner = s.strip_prefix(quote)?.strip_suffix(quote)?;
    Some(inner.to_owned())
}
