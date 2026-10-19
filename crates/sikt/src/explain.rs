//! Why a pattern didn't match: the attempt that got farthest.

use std::fmt;

/// The closest a pattern came to matching a text: how far it got, what it
/// expected there, and which captures it had matched by then.
///
/// Displays as a short report with the line and a caret:
///
/// ```text
/// no match. closest attempt on line 1:
///   expected digit but found "a"
///   12a
///     ^
/// ```
#[derive(Debug, Clone)]
pub struct Explanation<'a> {
    text: &'a str,
    start: usize,
    position: usize,
    expected: Vec<String>,
    matched: Vec<String>,
}

impl<'a> Explanation<'a> {
    pub(crate) fn new(
        text: &'a str,
        start: usize,
        position: usize,
        expected: Vec<String>,
        matched: Vec<String>,
    ) -> Self {
        Explanation {
            text,
            start,
            position,
            expected,
            matched,
        }
    }

    /// Byte offset where the closest attempt started.
    pub fn start(&self) -> usize {
        self.start
    }

    /// Byte offset where the closest attempt failed.
    pub fn position(&self) -> usize {
        self.position
    }

    /// What the pattern expected there, as written in the pattern.
    pub fn expected(&self) -> &[String] {
        &self.expected
    }

    /// Captures the attempt had matched before failing.
    pub fn matched(&self) -> &[String] {
        &self.matched
    }

    /// The 1-based line of [`position`](Self::position).
    pub fn line(&self) -> usize {
        self.text[..self.position].matches('\n').count() + 1
    }
}

impl fmt::Display for Explanation<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pos = self.position;
        let line_start = self.text[..pos].rfind('\n').map_or(0, |i| i + 1);
        let line_end = self.text[pos..]
            .find('\n')
            .map_or(self.text.len(), |i| pos + i);
        let line = self.text[line_start..line_end].trim_end_matches('\r');
        let found = match self.text[pos..].chars().next() {
            None => "the end of the text".to_owned(),
            Some('\n' | '\r') => "a line break".to_owned(),
            Some(c) => format!("{:?}", c.to_string()),
        };

        writeln!(f, "no match. closest attempt on line {}:", self.line())?;
        let lead = match self.matched.as_slice() {
            [] => String::new(),
            names => format!(
                "matched {}, then ",
                names
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        match self.expected.as_slice() {
            [] => writeln!(f, "  {lead}nothing could match at {found}")?,
            expected => writeln!(
                f,
                "  {lead}expected {} but found {found}",
                or_list(expected)
            )?,
        }
        writeln!(f, "  {line}")?;
        // Tabs in the padding keep the caret aligned under tabs in the line.
        let padding: String = self.text[line_start..pos]
            .chars()
            .map(|c| if c == '\t' { '\t' } else { ' ' })
            .collect();
        write!(f, "  {padding}^")
    }
}

/// `a`, `a or b`, `a, b or c`.
fn or_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use crate::Pattern;

    #[test]
    fn start_and_position() {
        let p = Pattern::new(r#""x" 3 digit"#).unwrap();
        let e = p.explain("ab x12a").unwrap().unwrap();
        assert_eq!((e.start(), e.position(), e.line()), (3, 6, 1));
    }
}
