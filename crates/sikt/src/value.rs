//! Typed capture values.

use crate::haystack::Haystack;

/// What a capture holds: text, or a number for captures of `integer` and
/// `number`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureType {
    Text,
    Integer,
    Number,
}

/// A capture's value. An `integer` too large for `i64` comes back as a
/// `Number`.
#[derive(Debug, PartialEq)]
pub enum Value<'a, H: Haystack + ?Sized = str> {
    Text(&'a H),
    Integer(i64),
    Number(f64),
}

impl<H: Haystack + ?Sized> Clone for Value<'_, H> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<H: Haystack + ?Sized> Copy for Value<'_, H> {}

impl CaptureType {
    pub(crate) fn name(self) -> &'static str {
        match self {
            CaptureType::Text => "text",
            CaptureType::Integer => "integer",
            CaptureType::Number => "number",
        }
    }

    /// The value of `text`, captured with this type.
    pub(crate) fn value<'a, H: Haystack + ?Sized>(self, text: &'a H) -> Value<'a, H> {
        let parsed = std::str::from_utf8(text.bytes()).ok();
        let integer = || {
            parsed?
                .trim_start_matches('+')
                .parse()
                .ok()
                .map(Value::Integer)
        };
        let number = || parsed?.parse().ok().map(Value::Number);
        let value = match self {
            CaptureType::Text => None,
            CaptureType::Integer => integer().or_else(number),
            CaptureType::Number => number(),
        };
        value.unwrap_or(Value::Text(text))
    }
}
