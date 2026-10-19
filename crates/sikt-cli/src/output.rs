//! Colors and diffs for the terminal.

use std::io::{self, Write};
use std::ops::Range;

/// ANSI styling, or none when color is off.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub color: bool,
}

impl Style {
    fn paint(
        self,
        out: &mut impl Write,
        code: &str,
        text: impl std::fmt::Display,
    ) -> io::Result<()> {
        if self.color {
            write!(out, "\x1b[{code}m{text}\x1b[0m")
        } else {
            write!(out, "{text}")
        }
    }

    pub fn path(self, out: &mut impl Write, text: &str) -> io::Result<()> {
        self.paint(out, "35", text)
    }

    pub fn line_number(self, out: &mut impl Write, n: usize) -> io::Result<()> {
        self.paint(out, "32", n)
    }

    pub fn matched(self, out: &mut impl Write, text: &str) -> io::Result<()> {
        self.paint(out, "1;31", text)
    }

    /// `line` with the byte `ranges` highlighted.
    pub fn highlight(
        self,
        out: &mut impl Write,
        line: &str,
        ranges: &[Range<usize>],
    ) -> io::Result<()> {
        let mut last = 0;
        for r in ranges {
            out.write_all(&line.as_bytes()[last..r.start])?;
            self.matched(out, &line[r.clone()])?;
            last = r.end;
        }
        out.write_all(&line.as_bytes()[last..])
    }

    /// A unified diff between two versions of the file at `path`.
    pub fn diff(self, path: &str, old: &str, new: &str) -> String {
        let paint = |code: &str, line: &str| format!("\x1b[{code}m{line}\x1b[0m");
        let diff = similar::TextDiff::from_lines(old, new);
        let text = diff
            .unified_diff()
            .header(&format!("a/{path}"), &format!("b/{path}"))
            .to_string();
        if !self.color {
            return text;
        }
        text.split_inclusive('\n')
            .map(|line| match line.as_bytes().first() {
                Some(b'+') if !line.starts_with("+++") => paint("32", line),
                Some(b'-') if !line.starts_with("---") => paint("31", line),
                Some(b'@') => paint("36", line),
                _ => line.to_owned(),
            })
            .collect()
    }
}

/// A pattern string that matches `text` literally.
pub fn quote(text: &str) -> String {
    let mut out = String::from('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_literals() {
        let quoted = quote("say \"hi\" \\o/\n");
        assert_eq!(quoted, r#""say \"hi\" \\o/\n""#);
        let p = sikt::Pattern::new(&quoted).unwrap();
        assert!(p.is_match("they say \"hi\" \\o/\n").unwrap());
    }
}
