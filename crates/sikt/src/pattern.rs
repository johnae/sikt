use std::borrow::Cow;
use std::fmt;
use std::ops::Range;

use crate::class::char_len;
use crate::compile::{Code, compile};
use crate::error::{Error, MatchError, Warning};
use crate::explain::Explanation;
use crate::haystack::Haystack;
use crate::lint::lint;
use crate::prefilter::{LeadingRun, Prefilter, RunPlan};
use crate::program::Program;
use crate::syntax::{Ast, parse};
use crate::template::Template;
use crate::vm::{self, Captures, Limits, Scratch, Trace};

const DEFAULT_LIMITS: Limits = Limits {
    steps: 10_000_000,
    depth: 1_000,
};

/// A compiled pattern. Immutable, and cheap to share between threads.
pub struct Pattern {
    source: String,
    program: Code,
    prefilter: Prefilter,
    leading_run: Option<LeadingRun>,
    limits: Limits,
    /// Matches stay within lines; see [`PatternBuilder::line_mode`].
    line_mode: bool,
    warnings: Vec<Warning>,
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
    line_mode: bool,
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

    /// Run the pattern on each line without its line break, as the `sikt`
    /// command does by default. Matches never cross lines, `text start` and
    /// `text end` match at the ends of each line, and offsets are still into
    /// the whole text. A line break at the end of the text doesn't start
    /// another line.
    pub fn line_mode(mut self, yes: bool) -> Self {
        self.line_mode = yes;
        self
    }

    pub fn build(&self, source: &str) -> Result<Pattern, Error> {
        let (pattern, ast) = self.compile(source)?;
        let statements = &ast.statements;
        let keyword = statements
            .filter
            .as_ref()
            .map(|(span, _)| (*span, "where"))
            .or(statements
                .output
                .as_ref()
                .map(|(span, out)| (*span, if out.print { "print" } else { "replace" })));
        if let Some((span, word)) = keyword {
            return Err(Error::new(
                source,
                format!("`{word}` only works in a program"),
                span,
                None,
            )
            .with_help("compile it with `Program::new`, or run it with the `sikt` command"));
        }
        Ok(pattern)
    }

    /// Compiles a program: a pattern with `where`, `replace` or `print`.
    pub fn build_program(&self, source: &str) -> Result<Program, Error> {
        let (pattern, mut ast) = self.compile(source)?;
        let statements = std::mem::take(&mut ast.statements);
        Program::from_parts(pattern, statements, &ast, source, self.case_insensitive)
    }

    fn compile(&self, source: &str) -> Result<(Pattern, Ast), Error> {
        let mut ast = parse(source)?;
        let warnings = lint(&ast, source, self.line_mode);
        if self.case_insensitive {
            ast.main = ast.main.fold_case();
        }
        let program = compile(&ast, source)?;
        let pattern = Pattern {
            source: source.to_owned(),
            program,
            prefilter: Prefilter::new(&ast.main),
            leading_run: LeadingRun::new(&ast.main),
            limits: self.limits,
            line_mode: self.line_mode,
            warnings,
        };
        Ok((pattern, ast))
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
            line_mode: false,
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Things in the pattern that compile but probably don't do what was meant.
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// Capture names in order of first appearance.
    pub fn capture_names(&self) -> impl Iterator<Item = &str> {
        self.program.captures.iter().map(String::as_str)
    }

    pub fn is_match(&self, text: &str) -> Result<bool, MatchError> {
        Ok(self.find(text)?.is_some())
    }

    /// Like [`is_match`](Self::is_match), on bytes.
    pub fn is_match_bytes(&self, text: &[u8]) -> Result<bool, MatchError> {
        Ok(self.find_bytes(text)?.is_some())
    }

    /// The first match, scanning left to right.
    pub fn find<'a>(&'a self, text: &'a str) -> Result<Option<Match<'a>>, MatchError> {
        self.find_iter(text).next().transpose()
    }

    /// Like [`find`](Self::find), on bytes.
    pub fn find_bytes<'a>(&'a self, text: &'a [u8]) -> Result<Option<Match<'a, [u8]>>, MatchError> {
        self.find_iter_bytes(text).next().transpose()
    }

    /// The text with every match replaced by `template`.
    pub fn replace_all<'a>(
        &self,
        text: &'a str,
        template: &Template,
    ) -> Result<Cow<'a, str>, MatchError> {
        self.replace_all_with(text, |m, out| template.render_into(m, out))
    }

    /// The text with every match replaced by what `replace` writes for it.
    pub fn replace_all_with<'a>(
        &self,
        text: &'a str,
        replace: impl FnMut(&Match, &mut String),
    ) -> Result<Cow<'a, str>, MatchError> {
        splice(text, self.find_iter(text), replace)
    }

    pub(crate) fn limits(&self) -> Limits {
        self.limits
    }

    /// Why the pattern matches nowhere in `text`: the attempt that got
    /// farthest, or `None` if there is a match. Tries every position, so it's
    /// slower than searching; meant for when a search came up empty.
    pub fn explain<'a>(&self, text: &'a str) -> Result<Option<Explanation<'a>>, MatchError> {
        let mut scratch = Scratch::new(&self.program);
        // The attempt that got farthest from where it started; the first wins ties.
        let mut best = Trace::default();
        let mut best_start = 0;
        let mut best_progress = None;
        for (window_start, window_end) in self.windows(text) {
            let hay = &text.as_bytes()[window_start..window_end];
            scratch.new_haystack();
            let mut pos = 0;
            while pos <= hay.len() {
                let mut trace = Trace::default();
                let found = vm::exec_traced(
                    &self.program,
                    hay,
                    pos,
                    &mut scratch,
                    self.limits,
                    &mut trace,
                )?;
                if found.is_some() {
                    return Ok(None);
                }
                let progress = trace.farthest.map(|f| f - pos);
                if progress > best_progress {
                    best_progress = progress;
                    best_start = window_start + pos;
                    trace.farthest = trace.farthest.map(|f| window_start + f);
                    best = trace;
                }
                pos += if pos < hay.len() {
                    char_len(&hay[pos..])
                } else {
                    1
                };
            }
        }
        let mut expected: Vec<String> = Vec::new();
        for span in best.failed.iter().filter_map(|&pc| self.program.spans[pc]) {
            let source = &self.source[span.start..span.end];
            if !expected.iter().any(|e| e == source) {
                expected.push(source.to_owned());
            }
        }
        let mut matched: Vec<String> = Vec::new();
        for &slot in &best.closed {
            let name = &self.program.captures[slot as usize];
            if !matched.contains(name) {
                matched.push(name.clone());
            }
        }
        Ok(Some(Explanation::new(
            text,
            best_start,
            best.farthest.unwrap_or(0),
            expected,
            matched,
        )))
    }

    /// The parts of `text` a match must stay within: each line in line mode,
    /// otherwise all of it. An empty text still has one, so it can be explained.
    fn windows(&self, text: &str) -> Vec<(usize, usize)> {
        if !self.line_mode || text.is_empty() {
            return vec![(0, text.len())];
        }
        let mut start = 0;
        text.split_inclusive('\n')
            .map(|chunk| {
                let line = chunk
                    .strip_suffix('\n')
                    .map_or(chunk, |l| l.strip_suffix('\r').unwrap_or(l));
                let window = (start, start + line.len());
                start += chunk.len();
                window
            })
            .collect()
    }

    /// All non-overlapping matches, left to right. After an empty match the
    /// scan moves on one character. Stops after the first error.
    pub fn find_iter<'a>(&'a self, text: &'a str) -> FindIter<'a> {
        self.search(text)
    }

    /// Like [`find_iter`](Self::find_iter), on bytes that may not be valid
    /// UTF-8. Invalid bytes match only `any`.
    pub fn find_iter_bytes<'a>(&'a self, text: &'a [u8]) -> FindIter<'a, [u8]> {
        self.search(text)
    }

    pub(crate) fn search<'a, H: Haystack + ?Sized>(&'a self, text: &'a H) -> FindIter<'a, H> {
        let bytes = text.bytes();
        FindIter {
            pattern: self,
            text,
            bytes,
            pos: 0,
            window: (!self.line_mode).then_some(0..bytes.len()),
            next_line: 0,
            scratch: Scratch::new(&self.program),
            done: false,
        }
    }
}

/// `text` with each of `matches` replaced by what `replace` writes for it.
pub(crate) fn splice<'a, 'm>(
    text: &'a str,
    matches: impl Iterator<Item = Result<Match<'m>, MatchError>>,
    mut replace: impl FnMut(&Match, &mut String),
) -> Result<Cow<'a, str>, MatchError> {
    let mut out = String::new();
    let mut last = 0;
    let mut replaced = false;
    for m in matches {
        let m = m?;
        out.push_str(&text[last..m.start()]);
        replace(&m, &mut out);
        last = m.end();
        replaced = true;
    }
    if !replaced {
        return Ok(Cow::Borrowed(text));
    }
    out.push_str(&text[last..]);
    Ok(Cow::Owned(out))
}

/// Iterator over the matches in a text. See [`Pattern::find_iter`].
#[derive(Debug)]
pub struct FindIter<'a, H: Haystack + ?Sized = str> {
    pattern: &'a Pattern,
    text: &'a H,
    bytes: &'a [u8],
    pos: usize,
    /// The part of the text being searched: all of it, or in line mode the
    /// current line without its line break. `None` between lines.
    window: Option<Range<usize>>,
    /// In line mode, where the line after the window starts.
    next_line: usize,
    scratch: Scratch,
    done: bool,
}

impl<'a, H: Haystack + ?Sized> Iterator for FindIter<'a, H> {
    type Item = Result<Match<'a, H>, MatchError>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.done {
            if self.window.is_some()
                && let Some(found) = self.search_window()
            {
                return Some(found);
            }
            if !self.pattern.line_mode || !self.enter_next_line() {
                self.done = true;
            }
        }
        None
    }
}

impl<'a, H: Haystack + ?Sized> FindIter<'a, H> {
    /// The next match in the window, or `None` once it has no more.
    fn search_window(&mut self) -> Option<Result<Match<'a, H>, MatchError>> {
        let window = self.window.clone().expect("searching a window");
        let base = window.start;
        let hay = &self.bytes[window];
        let program = &self.pattern.program;
        while let Some(candidate) = self.pattern.prefilter.find(hay, self.pos - base) {
            let plan = self
                .pattern
                .leading_run
                .as_ref()
                .map_or(RunPlan::Normal, |run| run.plan(hay, candidate));
            let (start, on_fail) = match plan {
                RunPlan::Normal => (candidate, None),
                RunPlan::Skip(end) => {
                    self.pos = base + end;
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
                    self.pos = base + if end > start { end } else { start + step };
                    let captures = self.scratch.capture_spans(base);
                    return Some(Ok(Match {
                        pattern: self.pattern,
                        text: self.text,
                        range: base + start..base + end,
                        captures,
                    }));
                }
                Ok(None) => self.pos = base + on_fail.unwrap_or(start + step),
            }
        }
        self.window = None;
        None
    }

    /// Moves the window to the next line where a match could start, using the
    /// prefilter over the rest of the text to skip lines. `false` at the end.
    fn enter_next_line(&mut self) -> bool {
        let bytes = self.bytes;
        let mut pos = self.next_line;
        // A line break at the very end doesn't start another line.
        while pos < bytes.len() {
            let Some(candidate) = self.pattern.prefilter.find(bytes, pos) else {
                return false;
            };
            let start = memchr::memrchr(b'\n', &bytes[pos..candidate]).map_or(pos, |i| pos + i + 1);
            let (end, next) = match memchr::memchr(b'\n', &bytes[candidate..]) {
                Some(i) => {
                    let newline = candidate + i;
                    let crlf = newline > start && bytes[newline - 1] == b'\r';
                    (if crlf { newline - 1 } else { newline }, newline + 1)
                }
                None => (bytes.len(), bytes.len()),
            };
            if candidate > end {
                // The candidate is the line break itself.
                pos = next;
                continue;
            }
            self.window = Some(start..end);
            self.next_line = next;
            self.pos = candidate;
            self.scratch.new_haystack();
            return true;
        }
        false
    }
}

/// One match, with its named captures. `H` is what was searched: `str`, or
/// `[u8]`, in which case the match and captures are bytes.
pub struct Match<'a, H: Haystack + ?Sized = str> {
    pattern: &'a Pattern,
    text: &'a H,
    range: Range<usize>,
    captures: Captures,
}

impl<H: Haystack + ?Sized> Clone for Match<'_, H> {
    fn clone(&self) -> Self {
        Match {
            pattern: self.pattern,
            text: self.text,
            range: self.range.clone(),
            captures: self.captures.clone(),
        }
    }
}

impl<H: Haystack + ?Sized> fmt::Debug for Match<'_, H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("Match");
        d.field("text", &self.text()).field("range", &self.range);
        for name in self.pattern.capture_names() {
            d.field(name, &self.get(name));
        }
        d.finish()
    }
}

impl<'a> Match<'a, str> {
    pub fn as_str(&self) -> &'a str {
        self.text()
    }
}

impl<'a> Match<'a, [u8]> {
    pub fn as_bytes(&self) -> &'a [u8] {
        self.text()
    }
}

impl<'a, H: Haystack + ?Sized> Match<'a, H> {
    /// The matched text, as a `str` or bytes like the text searched.
    pub fn text(&self) -> &'a H {
        self.text.part(self.range.clone())
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
    pub fn get(&self, name: &str) -> Option<&'a H> {
        self.span(name).map(|r| self.text.part(r))
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
    pub fn all(&self, name: &str) -> impl Iterator<Item = &'a H> {
        let slot = self.slot(name);
        let text = self.text;
        self.captures
            .iter()
            .filter(move |(s, _)| Some(*s) == slot)
            .map(move |(_, r)| text.part(r.clone()))
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

        #[test]
        fn line_mode_is_searching_each_line(src in pattern(), input in "[ab12x\\- é\n\r]{0,30}") {
            let Ok(whole) = Pattern::builder().line_mode(true).build(&src) else { return Ok(()) };
            let per_line = Pattern::new(&src).unwrap();
            let found: Vec<_> = whole.find_iter(&input).map(|m| m.unwrap().range()).collect();
            let mut expected = Vec::new();
            let mut start = 0;
            for chunk in input.split_inclusive('\n') {
                let line = chunk.strip_suffix('\n').map_or(chunk, |l| l.strip_suffix('\r').unwrap_or(l));
                expected.extend(per_line.find_iter(line).map(|m| { let r = m.unwrap().range(); start + r.start..start + r.end }));
                start += chunk.len();
            }
            prop_assert_eq!(found, expected, "pattern `{}`", src);
        }

        #[test]
        fn bytes_and_str_agree(src in pattern(), input in "[ab12x\\- é\n]{0,24}", line_mode: bool) {
            let Ok(p) = Pattern::builder().line_mode(line_mode).build(&src) else { return Ok(()) };
            let text: Vec<_> = p.find_iter(&input).map(|m| m.unwrap().range()).collect();
            let bytes: Vec<_> = p.find_iter_bytes(input.as_bytes()).map(|m| m.unwrap().range()).collect();
            prop_assert_eq!(text, bytes, "pattern `{}`", src);
        }

        #[test]
        fn any_bytes_are_safe(src in pattern(), input in prop::collection::vec(any::<u8>(), 0..40), line_mode: bool) {
            let Ok(p) = Pattern::builder().line_mode(line_mode).build(&src) else { return Ok(()) };
            let mut last = 0;
            for m in p.find_iter_bytes(&input) {
                let m = m.unwrap();
                prop_assert!(last <= m.start() && m.end() <= input.len());
                last = m.end();
            }
        }

        #[test]
        fn replacing_with_the_match_changes_nothing(src in pattern(), input in "[ab12x\\- é\n]{0,24}") {
            let Ok(p) = Pattern::new(&src) else { return Ok(()) };
            let template = Template::new("{match}", &p).unwrap();
            prop_assert_eq!(p.replace_all(&input, &template).unwrap(), input.as_str());
        }
    }
}
