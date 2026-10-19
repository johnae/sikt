//! Colors, line splitting and diffs for the terminal.

use std::ops::Range;

/// ANSI styling, or none when color is off.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub color: bool,
}

impl Style {
    fn paint(self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    pub fn path(self, text: &str) -> String {
        self.paint("35", text)
    }

    pub fn line_number(self, n: usize) -> String {
        self.paint("32", &n.to_string())
    }

    pub fn matched(self, text: &str) -> String {
        self.paint("1;31", text)
    }

    /// `line` with the byte `ranges` highlighted.
    pub fn highlight(self, line: &str, ranges: &[Range<usize>]) -> String {
        let mut out = String::new();
        let mut last = 0;
        for r in ranges {
            out.push_str(&line[last..r.start]);
            out.push_str(&self.matched(&line[r.clone()]));
            last = r.end;
        }
        out.push_str(&line[last..]);
        out
    }

    /// A unified diff between two versions of the file at `path`.
    pub fn diff(self, path: &str, old: &str, new: &str) -> String {
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
                Some(b'+') if !line.starts_with("+++") => self.paint("32", line),
                Some(b'-') if !line.starts_with("---") => self.paint("31", line),
                Some(b'@') => self.paint("36", line),
                _ => line.to_owned(),
            })
            .collect()
    }
}

/// One line of a text: its byte offset, its contents and its line break.
pub struct Line<'a> {
    pub start: usize,
    pub text: &'a str,
    pub ending: &'a str,
}

/// The lines of `text`, each without its `\n` or `\r\n`.
pub fn lines(text: &str) -> impl Iterator<Item = Line<'_>> {
    let mut start = 0;
    text.split_inclusive('\n').map(move |chunk| {
        let text = chunk
            .strip_suffix('\n')
            .map_or(chunk, |t| t.strip_suffix('\r').unwrap_or(t));
        let line = Line {
            start,
            text,
            ending: &chunk[text.len()..],
        };
        start += chunk.len();
        line
    })
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
    fn splits_lines_keeping_endings() {
        let parts: Vec<_> = lines("a\r\nb\nc")
            .map(|l| (l.start, l.text, l.ending))
            .collect();
        assert_eq!(parts, [(0, "a", "\r\n"), (3, "b", "\n"), (5, "c", "")]);
    }

    #[test]
    fn quotes_literals() {
        let quoted = quote("say \"hi\" \\o/\n");
        assert_eq!(quoted, r#""say \"hi\" \\o/\n""#);
        let p = sikt::Pattern::new(&quoted).unwrap();
        assert!(p.is_match("they say \"hi\" \\o/\n").unwrap());
    }
}
