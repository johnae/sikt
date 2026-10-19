use std::fmt;
use std::ops::Range;

use crate::class::char_len;
use crate::compile::{Program, compile};
use crate::error::{Error, MatchError};
use crate::syntax::parse;
use crate::vm::{self, CaptureEvent, Scratch};

const DEFAULT_MAX_STEPS: u64 = 10_000_000;

/// A compiled pattern. Immutable, and cheap to share between threads.
pub struct Pattern {
    source: String,
    program: Program,
    max_steps: u64,
}

impl fmt::Debug for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Pattern").field(&self.source).finish()
    }
}

/// Configures a [`Pattern`] before compiling it.
#[derive(Debug, Clone)]
pub struct PatternBuilder {
    max_steps: u64,
}

impl PatternBuilder {
    /// VM steps allowed for one match attempt at one position. Exceeding it
    /// returns [`MatchError::StepLimit`].
    pub fn max_steps(mut self, steps: u64) -> Self {
        self.max_steps = steps;
        self
    }

    pub fn build(&self, source: &str) -> Result<Pattern, Error> {
        let expr = parse(source)?;
        let program = compile(&expr, source)?;
        Ok(Pattern {
            source: source.to_owned(),
            program,
            max_steps: self.max_steps,
        })
    }
}

impl Pattern {
    pub fn new(source: &str) -> Result<Pattern, Error> {
        Pattern::builder().build(source)
    }

    pub fn builder() -> PatternBuilder {
        PatternBuilder {
            max_steps: DEFAULT_MAX_STEPS,
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Capture names in order of first appearance.
    pub fn capture_names(&self) -> impl Iterator<Item = &str> {
        self.program.captures.iter().map(String::as_str)
    }

    pub fn is_match(&self, text: &str) -> Result<bool, MatchError> {
        Ok(self.find(text)?.is_some())
    }

    /// The first match, scanning left to right.
    pub fn find<'a>(&'a self, text: &'a str) -> Result<Option<Match<'a>>, MatchError> {
        self.find_iter(text).next().transpose()
    }

    /// All non-overlapping matches, left to right. After an empty match the
    /// scan moves on one character. Stops after the first error.
    pub fn find_iter<'a>(&'a self, text: &'a str) -> FindIter<'a> {
        FindIter {
            pattern: self,
            text,
            pos: 0,
            scratch: Scratch::new(&self.program),
            done: false,
        }
    }
}

/// Iterator over the matches in a text. See [`Pattern::find_iter`].
#[derive(Debug)]
pub struct FindIter<'a> {
    pattern: &'a Pattern,
    text: &'a str,
    pos: usize,
    scratch: Scratch,
    done: bool,
}

impl<'a> Iterator for FindIter<'a> {
    type Item = Result<Match<'a>, MatchError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let hay = self.text.as_bytes();
        let program = &self.pattern.program;
        while self.pos <= hay.len() {
            let start = self.pos;
            let step = if start < hay.len() {
                char_len(&hay[start..])
            } else {
                1
            };
            match vm::exec(
                program,
                hay,
                start,
                &mut self.scratch,
                self.pattern.max_steps,
            ) {
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
                Ok(Some(end)) => {
                    self.pos = if end > start { end } else { start + step };
                    let captures = collect_captures(&self.scratch.captures);
                    return Some(Ok(Match {
                        pattern: self.pattern,
                        text: self.text,
                        range: start..end,
                        captures,
                    }));
                }
                Ok(None) => self.pos = start + step,
            }
        }
        self.done = true;
        None
    }
}

/// Pairs open and close events into `(slot, range)`, in closing order.
fn collect_captures(events: &[CaptureEvent]) -> Vec<(u32, Range<usize>)> {
    let mut open = Vec::new();
    let mut out = Vec::new();
    for event in events {
        if event.open {
            open.push(event.pos);
        } else {
            let start = open.pop().expect("captures are nested");
            out.push((event.slot, start..event.pos));
        }
    }
    out
}

/// One match, with its named captures.
#[derive(Clone)]
pub struct Match<'a> {
    pattern: &'a Pattern,
    text: &'a str,
    range: Range<usize>,
    captures: Vec<(u32, Range<usize>)>,
}

impl fmt::Debug for Match<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("Match");
        d.field("text", &self.as_str()).field("range", &self.range);
        for name in self.pattern.capture_names() {
            d.field(name, &self.get(name));
        }
        d.finish()
    }
}

impl<'a> Match<'a> {
    pub fn as_str(&self) -> &'a str {
        &self.text[self.range.clone()]
    }

    /// Byte offsets of the match.
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    pub fn start(&self) -> usize {
        self.range.start
    }

    pub fn end(&self) -> usize {
        self.range.end
    }

    /// The last text captured under `name`, or `None` if it didn't participate.
    pub fn get(&self, name: &str) -> Option<&'a str> {
        self.span(name).map(|r| &self.text[r])
    }

    /// Byte offsets of the last text captured under `name`.
    pub fn span(&self, name: &str) -> Option<Range<usize>> {
        let slot = self.slot(name)?;
        self.captures
            .iter()
            .rev()
            .find(|(s, _)| *s == slot)
            .map(|(_, r)| r.clone())
    }

    /// Every text captured under `name`, in order. Captures inside a
    /// repetition record each occurrence.
    pub fn all(&self, name: &str) -> impl Iterator<Item = &'a str> {
        let slot = self.slot(name);
        let text = self.text;
        self.captures
            .iter()
            .filter(move |(s, _)| Some(*s) == slot)
            .map(move |(_, r)| &text[r.clone()])
    }

    fn slot(&self, name: &str) -> Option<u32> {
        self.pattern
            .program
            .captures
            .iter()
            .position(|n| n == name)
            .map(|s| s as u32)
    }
}
