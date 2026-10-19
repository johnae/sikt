use std::fmt;
use std::ops::Range;

use crate::class::char_len;
use crate::compile::{Program, compile};
use crate::error::{Error, MatchError};
use crate::prefilter::{LeadingRun, Prefilter, RunPlan};
use crate::syntax::parse;
use crate::vm::{self, CaptureEvent, Limits, Scratch};

const DEFAULT_LIMITS: Limits = Limits {
    steps: 10_000_000,
    depth: 1_000,
};

/// A compiled pattern. Immutable, and cheap to share between threads.
pub struct Pattern {
    source: String,
    program: Program,
    prefilter: Prefilter,
    leading_run: Option<LeadingRun>,
    limits: Limits,
}

impl fmt::Debug for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Pattern").field(&self.source).finish()
    }
}

/// Configures a [`Pattern`] before compiling it.
#[derive(Debug, Clone)]
pub struct PatternBuilder {
    limits: Limits,
    case_insensitive: bool,
}

impl PatternBuilder {
    /// VM steps allowed for one match attempt at one position. Exceeding it
    /// returns [`MatchError::StepLimit`].
    pub fn max_steps(mut self, steps: u64) -> Self {
        self.limits.steps = steps;
        self
    }

    /// How deeply rules may call each other. Exceeding it returns
    /// [`MatchError::DepthLimit`].
    pub fn max_depth(mut self, depth: u32) -> Self {
        self.limits.depth = depth;
        self
    }

    /// Match the whole pattern case-insensitively, as if it were wrapped in
    /// `nocase`.
    pub fn case_insensitive(mut self, yes: bool) -> Self {
        self.case_insensitive = yes;
        self
    }

    pub fn build(&self, source: &str) -> Result<Pattern, Error> {
        let mut ast = parse(source)?;
        if self.case_insensitive {
            ast.main = ast.main.fold_case();
        }
        let program = compile(&ast, source)?;
        Ok(Pattern {
            source: source.to_owned(),
            program,
            prefilter: Prefilter::new(&ast.main),
            leading_run: LeadingRun::new(&ast.main),
            limits: self.limits,
        })
    }
}

impl Pattern {
    pub fn new(source: &str) -> Result<Pattern, Error> {
        Pattern::builder().build(source)
    }

    pub fn builder() -> PatternBuilder {
        PatternBuilder {
            limits: DEFAULT_LIMITS,
            case_insensitive: false,
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
        while let Some(candidate) = self.pattern.prefilter.find(hay, self.pos) {
            let plan = self
                .pattern
                .leading_run
                .as_ref()
                .map_or(RunPlan::Normal, |run| run.plan(hay, candidate));
            let (start, on_fail) = match plan {
                RunPlan::Normal => (candidate, None),
                RunPlan::Skip(end) => {
                    self.pos = end;
                    continue;
                }
                RunPlan::Attempt { start, on_fail } => (start, Some(on_fail)),
            };
            let step = if start < hay.len() {
                char_len(&hay[start..])
            } else {
                1
            };
            match vm::exec(program, hay, start, &mut self.scratch, self.pattern.limits) {
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
                Ok(None) => self.pos = on_fail.unwrap_or(start + step),
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

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// Matches by attempting at every position, without prefilters or run
    /// skipping.
    fn naive(p: &Pattern, text: &str) -> Vec<Range<usize>> {
        let hay = text.as_bytes();
        let mut scratch = Scratch::new(&p.program);
        let mut out = Vec::new();
        let mut pos = 0;
        while pos <= hay.len() {
            let step = if pos < hay.len() {
                char_len(&hay[pos..])
            } else {
                1
            };
            match vm::exec(&p.program, hay, pos, &mut scratch, p.limits).unwrap() {
                Some(end) => {
                    out.push(pos..end);
                    pos = if end > pos { end } else { pos + step };
                }
                None => pos += step,
            }
        }
        out
    }

    fn pattern() -> impl Strategy<Value = String> {
        let atom = prop::sample::select(vec![
            r#""a""#,
            r#""ab""#,
            r#""-""#,
            r#""é""#,
            "digit",
            "letter",
            "wordchar",
            "space",
            "any",
            "newline",
            r#"("a" or digit)"#,
            r#"until "-""#,
            "through digit",
            r#"one of "ab-""#,
            r#"none of (digit "\n")"#,
            "line start",
            "word end",
            r#"not followed by "a""#,
            "after digit",
            r#"nocase "ab""#,
        ]);
        let prefix = prop::sample::select(vec![
            "",
            "",
            "1+ ",
            "0+ ",
            "2 ",
            "2..3 ",
            "optional ",
            "n: ",
        ]);
        prop::collection::vec((prefix, atom), 1..4).prop_map(|items| {
            items
                .iter()
                .map(|(q, a)| format!("{q}{a}"))
                .collect::<Vec<_>>()
                .join(" ")
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(3000))]

        #[test]
        fn optimized_scan_matches_naive_scan(src in pattern(), input in "[ab12x\\- é\n]{0,24}") {
            let Ok(p) = Pattern::new(&src) else { return Ok(()) };
            let fast: Vec<_> = p.find_iter(&input).map(|m| m.unwrap().range()).collect();
            prop_assert_eq!(fast, naive(&p, &input), "pattern `{}`", src);
        }
    }
}
