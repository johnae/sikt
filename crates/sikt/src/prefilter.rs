//! Finds candidate start positions so the VM doesn't run at every byte.
//!
//! Built from the AST: the literal every match starts with, or the set of
//! bytes a match can start with. Patterns that can match empty, or start with
//! `any`, get no prefilter.

use memchr::memmem::Finder;

use crate::class::Kind;
use crate::syntax::{Expr, ExprKind};

#[derive(Debug)]
pub(crate) enum Prefilter {
    None,
    Byte(u8),
    Byte2(u8, u8),
    Byte3(u8, u8, u8),
    /// A literal prefix of at least two bytes.
    Literal(Box<Finder<'static>>),
    Set(Box<ByteSet>),
}

impl Prefilter {
    pub fn new(expr: &Expr) -> Self {
        let (prefix, _) = literal_prefix(expr);
        if prefix.len() >= 2 {
            return Prefilter::Literal(Box::new(Finder::new(prefix.as_bytes()).into_owned()));
        }
        let (set, nullable) = first_bytes(expr);
        if nullable || set.is_full() {
            return Prefilter::None;
        }
        match set.bytes().collect::<Vec<_>>()[..] {
            [a] => Prefilter::Byte(a),
            [a, b] => Prefilter::Byte2(a, b),
            [a, b, c] => Prefilter::Byte3(a, b, c),
            _ => Prefilter::Set(Box::new(set)),
        }
    }

    /// The first candidate start at or after `pos`, or `None` if no match can
    /// start there or later.
    #[inline]
    pub fn find(&self, hay: &[u8], pos: usize) -> Option<usize> {
        let rest = hay.get(pos..)?;
        let found = match self {
            Prefilter::None => return Some(pos),
            Prefilter::Byte(a) => memchr::memchr(*a, rest),
            Prefilter::Byte2(a, b) => memchr::memchr2(*a, *b, rest),
            Prefilter::Byte3(a, b, c) => memchr::memchr3(*a, *b, *c, rest),
            Prefilter::Literal(finder) => finder.find(rest),
            Prefilter::Set(set) => rest.iter().position(|&b| set.contains(b)),
        };
        found.map(|i| pos + i)
    }
}

/// UTF-8 leading bytes of multibyte characters.
const NON_ASCII_LEADS: std::ops::RangeInclusive<u8> = 0xC2..=0xF4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ByteSet([bool; 256]);

impl ByteSet {
    fn empty() -> Self {
        ByteSet([false; 256])
    }

    fn full() -> Self {
        ByteSet([true; 256])
    }

    fn of(bytes: &[u8]) -> Self {
        let mut set = ByteSet::empty();
        for &b in bytes {
            set.insert(b);
        }
        set
    }

    fn insert(&mut self, b: u8) {
        self.0[b as usize] = true;
    }

    fn union(&mut self, other: &ByteSet) {
        for (a, b) in self.0.iter_mut().zip(other.0) {
            *a |= b;
        }
    }

    fn contains(&self, b: u8) -> bool {
        self.0[b as usize]
    }

    fn is_full(&self) -> bool {
        self.0.iter().all(|&b| b)
    }

    fn bytes(&self) -> impl Iterator<Item = u8> + '_ {
        (0..=255).filter(|&b| self.contains(b))
    }

    fn of_kind(kind: Kind) -> Self {
        let mut set = ByteSet::empty();
        for b in 0..0x80u8 {
            if kind.matches(b as char) {
                set.insert(b);
            }
        }
        if !matches!(kind, Kind::Digit | Kind::Hex | Kind::Punct) {
            NON_ASCII_LEADS.for_each(|b| set.insert(b));
        }
        set
    }
}

/// Bytes a match can start with, and whether it can match empty.
///
/// The set only holds bytes that start a character, so on valid UTF-8 every
/// position it finds is a character boundary. Only `any` (which also matches
/// invalid bytes) adds continuation bytes, and it makes the set full.
fn first_bytes(e: &Expr) -> (ByteSet, bool) {
    match &e.kind {
        ExprKind::Literal(s) => match s.as_bytes().first() {
            None => (ByteSet::empty(), true),
            Some(&b) => (ByteSet::of(&[b]), false),
        },
        ExprKind::Any | ExprKind::Until(_) | ExprKind::Through(_) => (ByteSet::full(), true),
        ExprKind::Newline => (ByteSet::of(b"\r\n"), false),
        ExprKind::Kind(kind) => (ByteSet::of_kind(*kind), false),
        ExprKind::Seq(items) => {
            let mut set = ByteSet::empty();
            for item in items {
                let (first, nullable) = first_bytes(item);
                set.union(&first);
                if !nullable {
                    return (set, false);
                }
            }
            (set, true)
        }
        ExprKind::Choice(alts) => {
            let mut set = ByteSet::empty();
            let mut any_nullable = false;
            for alt in alts {
                let (first, nullable) = first_bytes(alt);
                set.union(&first);
                any_nullable |= nullable;
            }
            (set, any_nullable)
        }
        ExprKind::Repeat { min, body, .. } => {
            let (set, nullable) = first_bytes(body);
            (set, nullable || *min == 0)
        }
        ExprKind::Capture { body, .. } => first_bytes(body),
    }
}

/// The literal every match starts with, and whether that literal is the whole
/// of `e` (so a following item can extend it).
fn literal_prefix(e: &Expr) -> (String, bool) {
    match &e.kind {
        ExprKind::Literal(s) => (s.clone(), true),
        ExprKind::Seq(items) => {
            let mut prefix = String::new();
            for item in items {
                let (p, complete) = literal_prefix(item);
                prefix.push_str(&p);
                if !complete {
                    return (prefix, false);
                }
            }
            (prefix, true)
        }
        ExprKind::Capture { body, .. } => literal_prefix(body),
        ExprKind::Repeat { min, body, .. } if *min >= 1 => (literal_prefix(body).0, false),
        _ => (String::new(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse;

    fn prefilter(src: &str) -> String {
        match Prefilter::new(&parse(src).unwrap()) {
            Prefilter::None => "none".into(),
            Prefilter::Byte(a) => format!("byte {:?}", a as char),
            Prefilter::Byte2(a, b) => format!("bytes {:?} {:?}", a as char, b as char),
            Prefilter::Byte3(a, b, c) => {
                format!("bytes {:?} {:?} {:?}", a as char, b as char, c as char)
            }
            Prefilter::Literal(f) => format!("literal {:?}", String::from_utf8_lossy(f.needle())),
            Prefilter::Set(set) => format!("set of {}", set.bytes().count()),
        }
    }

    #[test]
    fn literal_prefixes() {
        assert_eq!(prefilter(r#""ERROR" ":""#), r#"literal "ERROR:""#);
        assert_eq!(prefilter(r#"tag: ("<" "b") digit"#), r#"literal "<b""#);
        assert_eq!(prefilter(r#""ab" 1+ "c""#), r#"literal "abc""#);
        assert_eq!(prefilter(r#""ab" until "c""#), r#"literal "ab""#);
    }

    #[test]
    fn first_byte_sets() {
        assert_eq!(prefilter(r#""x" digit"#), "byte 'x'");
        assert_eq!(prefilter(r#""GET" or "POST""#), "bytes 'G' 'P'");
        assert_eq!(prefilter("newline"), r"bytes '\n' '\r'");
        assert_eq!(prefilter("digit"), "set of 10");
        assert_eq!(prefilter(r#"optional "-" digit"#), "set of 11");
        // ASCII letters plus every multibyte leading byte.
        assert_eq!(
            prefilter("letter"),
            format!("set of {}", 52 + NON_ASCII_LEADS.count())
        );
    }

    #[test]
    fn no_prefilter() {
        assert_eq!(prefilter(r#"optional "a""#), "none");
        assert_eq!(prefilter(r#"until ":""#), "none");
        assert_eq!(prefilter(r#"any "x""#), "none");
        assert_eq!(prefilter(r#""" or "a""#), "none");
    }
}
