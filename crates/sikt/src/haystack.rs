//! The kinds of text a pattern can search.

use std::fmt::Debug;
use std::ops::Range;

/// Text a pattern can search: `str`, or `[u8]` that may not be valid UTF-8.
/// Matches and captures come back as the same type. Invalid UTF-8 bytes
/// match only `any`.
///
/// Implemented for `str` and `[u8]` only.
pub trait Haystack: sealed::Sealed + Debug {
    #[doc(hidden)]
    fn bytes(&self) -> &[u8];

    /// The part of the text at `range`, which a match produced.
    #[doc(hidden)]
    fn part(&self, range: Range<usize>) -> &Self;
}

impl Haystack for str {
    fn bytes(&self) -> &[u8] {
        self.as_bytes()
    }

    fn part(&self, range: Range<usize>) -> &str {
        // Matches start and end on character boundaries in valid UTF-8.
        &self[range]
    }
}

impl Haystack for [u8] {
    fn bytes(&self) -> &[u8] {
        self
    }

    fn part(&self, range: Range<usize>) -> &[u8] {
        &self[range]
    }
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for str {}
    impl Sealed for [u8] {}
}
