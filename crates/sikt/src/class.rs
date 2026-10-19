//! Character kinds, character sets and UTF-8 decoding.

use std::cmp::Ordering;

/// A named kind of character, like `digit` or `letter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Digit,
    Hex,
    Letter,
    Upper,
    Lower,
    WordChar,
    Space,
    Punct,
}

impl Kind {
    pub(crate) const ALL: [Kind; 8] = [
        Kind::Digit,
        Kind::Hex,
        Kind::Letter,
        Kind::Upper,
        Kind::Lower,
        Kind::WordChar,
        Kind::Space,
        Kind::Punct,
    ];

    pub(crate) fn from_name(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == name)
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Kind::Digit => "digit",
            Kind::Hex => "hex",
            Kind::Letter => "letter",
            Kind::Upper => "upper",
            Kind::Lower => "lower",
            Kind::WordChar => "wordchar",
            Kind::Space => "space",
            Kind::Punct => "punct",
        }
    }

    pub(crate) fn matches(self, c: char) -> bool {
        match self {
            Kind::Digit => c.is_ascii_digit(),
            Kind::Hex => c.is_ascii_hexdigit(),
            Kind::Letter => c.is_alphabetic(),
            Kind::Upper => c.is_uppercase(),
            Kind::Lower => c.is_lowercase(),
            Kind::WordChar => c.is_alphabetic() || c.is_ascii_digit() || c == '_',
            Kind::Space => c.is_whitespace() && c != '\n' && c != '\r',
            Kind::Punct => c.is_ascii_punctuation(),
        }
    }
}

/// A set of characters: ranges and kinds, possibly negated. Membership of
/// ASCII characters is precomputed as a bitmap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CharSet {
    /// Sorted and non-overlapping.
    ranges: Vec<(char, char)>,
    kinds: Vec<Kind>,
    negated: bool,
    ascii: u128,
}

impl CharSet {
    pub(crate) fn new(mut ranges: Vec<(char, char)>, kinds: Vec<Kind>, negated: bool) -> Self {
        ranges.sort_unstable();
        let mut merged: Vec<(char, char)> = Vec::with_capacity(ranges.len());
        for (lo, hi) in ranges {
            match merged.last_mut() {
                Some(last) if lo as u32 <= last.1 as u32 + 1 => last.1 = last.1.max(hi),
                _ => merged.push((lo, hi)),
            }
        }
        let mut set = CharSet {
            ranges: merged,
            kinds,
            negated,
            ascii: 0,
        };
        set.ascii = (0..128u8)
            .filter(|&b| set.matches_slow(b as char))
            .fold(0, |bits, b| bits | 1 << b);
        set
    }

    pub(crate) fn kind(kind: Kind) -> Self {
        CharSet::new(Vec::new(), vec![kind], false)
    }

    /// The kind this set is, if it's exactly one kind.
    #[cfg(test)]
    pub(crate) fn as_kind(&self) -> Option<Kind> {
        match self.kinds[..] {
            [kind] if self.ranges.is_empty() && !self.negated => Some(kind),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn matches(&self, c: char) -> bool {
        if c.is_ascii() {
            self.ascii >> c as u32 & 1 == 1
        } else {
            self.matches_slow(c)
        }
    }

    fn matches_slow(&self, c: char) -> bool {
        let in_range = self
            .ranges
            .binary_search_by(|&(lo, hi)| {
                if hi < c {
                    Ordering::Less
                } else if lo > c {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            })
            .is_ok();
        (in_range || self.kinds.iter().any(|k| k.matches(c))) != self.negated
    }

    /// Whether any character outside ASCII can match.
    pub(crate) fn may_match_non_ascii(&self) -> bool {
        self.negated
            || self.ranges.last().is_some_and(|&(_, hi)| !hi.is_ascii())
            || self
                .kinds
                .iter()
                .any(|k| !matches!(k, Kind::Digit | Kind::Hex | Kind::Punct))
    }
}

/// Decodes the character at the start of `bytes`, which must not be empty.
///
/// An invalid UTF-8 sequence decodes as `None` with length 1: one byte that
/// only `any` matches.
#[inline]
pub(crate) fn decode(bytes: &[u8]) -> (Option<char>, usize) {
    let b0 = bytes[0];
    if b0 < 0x80 {
        return (Some(b0 as char), 1);
    }
    let len = match b0 {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return (None, 1),
    };
    match bytes.get(..len).map(std::str::from_utf8) {
        Some(Ok(s)) => (s.chars().next(), len),
        _ => (None, 1),
    }
}

/// Length in bytes of the character at the start of `bytes`.
#[inline]
pub(crate) fn char_len(bytes: &[u8]) -> usize {
    if bytes[0] < 0x80 { 1 } else { decode(bytes).1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_ascii_multibyte_and_invalid() {
        assert_eq!(decode(b"a"), (Some('a'), 1));
        assert_eq!(decode("é".as_bytes()), (Some('é'), 2));
        assert_eq!(decode("😀".as_bytes()), (Some('😀'), 4));
        assert_eq!(decode(&[0xFF, b'a']), (None, 1));
        // Truncated sequence and a lone continuation byte.
        assert_eq!(decode(&[0xE2, 0x82]), (None, 1));
        assert_eq!(decode(&[0x82]), (None, 1));
    }

    #[test]
    fn space_excludes_line_breaks() {
        assert!(Kind::Space.matches(' '));
        assert!(Kind::Space.matches('\t'));
        assert!(!Kind::Space.matches('\n'));
        assert!(!Kind::Space.matches('\r'));
    }

    #[test]
    fn sets_merge_ranges_and_negate() {
        let set = CharSet::new(
            vec![('c', 'e'), ('a', 'c'), ('x', 'x')],
            vec![Kind::Digit],
            false,
        );
        assert_eq!(set.ranges, [('a', 'e'), ('x', 'x')]);
        assert!(set.matches('b') && set.matches('7') && set.matches('x'));
        assert!(!set.matches('f') && !set.matches('é'));
        assert!(!set.may_match_non_ascii());

        let none = CharSet::new(vec![('α', 'ω')], vec![], true);
        assert!(!none.matches('λ') && none.matches('a') && none.matches('é'));
        assert!(none.may_match_non_ascii());
    }

    #[test]
    fn digit_is_ascii_only() {
        assert!(Kind::Digit.matches('7'));
        assert!(!Kind::Digit.matches('٣'));
    }
}
