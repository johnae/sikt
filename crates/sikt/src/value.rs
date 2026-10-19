//! Typed capture values.

use crate::haystack::Haystack;

/// What a capture holds: text, or a number for captures of `integer`,
/// `number`, `size` and `percent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureType {
    Text,
    Integer,
    Number,
    /// A size like `1.5G`, as an integer number of bytes.
    Size,
    /// A percentage like `48%`, as the number before the `%`.
    Percent,
    /// A duration like `1h30m` or `26:58`, as a number of seconds.
    Duration,
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
            CaptureType::Size => "size",
            CaptureType::Percent => "percent",
            CaptureType::Duration => "duration",
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
            CaptureType::Size => parsed.and_then(size).map(Value::Integer),
            CaptureType::Duration => parsed.and_then(duration).map(Value::Number),
            CaptureType::Percent => parsed
                .and_then(|t| t.strip_suffix('%')?.parse().ok())
                .map(Value::Number),
        };
        value.unwrap_or(Value::Text(text))
    }
}

/// Bytes in a size like `1.5G`, `512` or `20MiB`, rounded.
pub(crate) fn size(text: &str) -> Option<i64> {
    let split = text
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let bytes = number.parse::<f64>().ok()? * unit_factor(unit)?;
    Some(bytes.round() as i64)
}

/// Bytes per unit. Bare `K`, `M`, `G`... mean 1024 like `ls -h` and `df -h`;
/// so do `KiB`, `MiB`...; `KB`, `MB`, `kB`... mean 1000. No unit or `B` is
/// one byte.
pub(crate) fn unit_factor(unit: &str) -> Option<f64> {
    if unit.is_empty() || unit == "B" {
        return Some(1.0);
    }
    let mut chars = unit.chars();
    let power = match chars.next()?.to_ascii_uppercase() {
        'K' => 1,
        'M' => 2,
        'G' => 3,
        'T' => 4,
        'P' => 5,
        'E' => 6,
        _ => return None,
    };
    let base: f64 = match chars.as_str() {
        "" | "i" | "iB" => 1024.0,
        "B" => 1000.0,
        _ => return None,
    };
    Some(base.powi(power))
}

/// Seconds in a duration: units like `1h30m`, `250ms` and `2d`, or a clock like
/// `26:58` (minutes and seconds), `00:05:12` or `1-02:03:04` (with days).
pub(crate) fn duration(text: &str) -> Option<f64> {
    if text.contains(':') {
        let (days, clock) = match text.split_once('-') {
            Some((days, clock)) => (days.parse::<f64>().ok()?, clock),
            None => (0.0, text),
        };
        let parts: Vec<f64> = clock
            .split(':')
            .map(|p| p.parse().ok())
            .collect::<Option<_>>()?;
        let seconds = match parts[..] {
            [m, s] => m * 60.0 + s,
            [h, m, s] => h * 3600.0 + m * 60.0 + s,
            _ => return None,
        };
        return Some(days * 86400.0 + seconds);
    }
    let mut rest = text;
    let mut seconds = 0.0;
    while !rest.is_empty() {
        let number_end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
        let unit_end = rest[number_end..]
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .map_or(rest.len(), |i| number_end + i);
        let number: f64 = rest[..number_end].parse().ok()?;
        seconds += number * seconds_per(&rest[number_end..unit_end])?;
        rest = &rest[unit_end..];
    }
    Some(seconds)
}

/// Seconds per duration unit.
pub(crate) fn seconds_per(unit: &str) -> Option<f64> {
    Some(match unit {
        "ns" => 1e-9,
        "us" | "µs" => 1e-6,
        "ms" => 1e-3,
        "s" => 1.0,
        "m" => 60.0,
        "h" => 3600.0,
        "d" => 86400.0,
        "w" => 604800.0,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(duration("1h30m"), Some(5400.0));
        assert_eq!(duration("250ms"), Some(0.25));
        assert_eq!(duration("1.5h"), Some(5400.0));
        assert_eq!(duration("26:58"), Some(1618.0));
        assert_eq!(duration("1-02:03:04"), Some(93784.0));
        assert_eq!(duration("5min"), None);
        assert_eq!(duration("1:2:3:4"), None);
    }

    #[test]
    fn sizes() {
        assert_eq!(size("16G"), Some(16 << 30));
        assert_eq!(size("4.0K"), Some(4096));
        assert_eq!(size("1.5GiB"), Some(3 << 29));
        assert_eq!(size("2MB"), Some(2_000_000));
        assert_eq!(size("3kB"), Some(3000));
        assert_eq!(size("512"), Some(512));
        assert_eq!(size("0B"), Some(0));
        assert_eq!(size("1X"), None);
        assert_eq!(size("1GBx"), None);
    }
}
