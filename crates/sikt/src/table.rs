//! `table` programs: the header line names the columns, and each row below it
//! becomes captures named after them.

use std::collections::HashMap;
use std::fmt;

use crate::class::{CharSet, Kind};
use crate::error::Error;
use crate::pattern::{Pattern, PatternBuilder};
use crate::program::Program;
use crate::syntax::{Anchor, Ast, Expr, ExprKind, Statements, TableSpec};

/// The columns of a `table` program. Each header gets its own [`Program`] for
/// the rows below it, from [`for_header`](Self::for_header).
pub struct Table {
    spec: TableSpec,
    /// `where` and `print`, checked once a header names the columns.
    statements: Statements,
    rules: Vec<Expr>,
    source: String,
    builder: PatternBuilder,
}

impl fmt::Debug for Table {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Table").field(&self.source).finish()
    }
}

impl Table {
    pub(crate) fn new(
        spec: TableSpec,
        statements: Statements,
        ast: &Ast,
        source: &str,
        builder: &PatternBuilder,
    ) -> Self {
        Table {
            spec,
            statements,
            rules: ast.rules.clone(),
            source: source.to_owned(),
            builder: builder.clone().line_mode(true),
        }
    }

    /// The program for the rows under `header`: it matches a whole line and
    /// captures each column under its name. Errors when the program uses a
    /// column the header doesn't have.
    pub fn for_header(&self, header: &str) -> Result<Program, Error> {
        let names = self.names(header.trim_end_matches(['\r', '\n']))?;
        for (name, span) in self
            .spec
            .types
            .iter()
            .map(|(n, s, _)| (n, s))
            .chain(self.spec.with.iter().map(|(n, s, _)| (n, s)))
        {
            if !names.contains(name) {
                let error = Error::new(
                    &self.source,
                    format!("this header has no column `{name}`"),
                    *span,
                    None,
                );
                return Err(error.with_help(format!("its columns are {}", names.join(", "))));
            }
        }
        let ast = Ast {
            main: self.row(&names),
            rules: self.rules.clone(),
            unused_rules: Vec::new(),
            statements: self.statements.clone(),
        };
        let (pattern, mut ast) = self.builder.compile_ast(ast, &self.source, Vec::new())?;
        let statements = std::mem::take(&mut ast.statements);
        Program::from_parts(pattern, statements, &ast, &self.source, &self.builder)
    }

    /// Capture names for the columns: from `columns (...)`, or the header
    /// split by the separator.
    fn names(&self, header: &str) -> Result<Vec<String>, Error> {
        let raw: Vec<String> = match (&self.spec.columns, &self.spec.split) {
            (Some(columns), _) => columns.clone(),
            (None, None) => header.split_whitespace().map(str::to_owned).collect(),
            (None, Some(split)) => {
                let ast = Ast {
                    main: split.clone(),
                    rules: self.rules.clone(),
                    unused_rules: Vec::new(),
                    statements: Statements::default(),
                };
                let (separator, _) = self.builder.clone().line_mode(false).compile_ast(
                    ast,
                    &self.source,
                    Vec::new(),
                )?;
                self.split_by(&separator, header)?
            }
        };
        if raw.is_empty() {
            return Err(Error::new(
                &self.source,
                "the header has no columns",
                self.spec.span,
                None,
            ));
        }
        // Repeated names get `_2`, `_3`, ...
        let mut seen = HashMap::new();
        Ok(raw
            .iter()
            .enumerate()
            .map(|(i, column)| {
                let name = capture_name(column, i);
                let count = seen.entry(name.clone()).or_insert(0);
                *count += 1;
                if *count == 1 {
                    name
                } else {
                    format!("{name}_{count}")
                }
            })
            .collect())
    }

    /// The parts of `header` between matches of `separator`, without empty ones.
    fn split_by(&self, separator: &Pattern, header: &str) -> Result<Vec<String>, Error> {
        let mut parts = Vec::new();
        let mut last = 0;
        for m in separator.find_iter(header) {
            let m = m.map_err(|e| {
                Error::new(
                    &self.source,
                    format!("splitting the header: {e}"),
                    self.spec.span,
                    None,
                )
            })?;
            parts.push(&header[last..m.start()]);
            last = m.end();
        }
        parts.push(&header[last..]);
        Ok(parts
            .into_iter()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// One row: the columns in order, separated by the separator. Trailing
    /// columns may be missing, and the last column takes the rest of the line.
    fn row(&self, names: &[String]) -> Expr {
        let span = self.spec.span;
        let expr = |kind| Expr::new(kind, span);
        let separator = self.spec.split.clone().unwrap_or_else(|| {
            let space = expr(ExprKind::Class(CharSet::kind(Kind::Space)));
            expr(ExprKind::Repeat {
                min: 1,
                max: None,
                body: Box::new(space),
            })
        });
        let optional = |e: Expr| {
            expr(ExprKind::Repeat {
                min: 0,
                max: Some(1),
                body: Box::new(e),
            })
        };
        let text_end = || expr(ExprKind::Anchor(Anchor::TextEnd));
        let stop = expr(ExprKind::Choice(vec![separator.clone(), text_end()]));

        let mut row: Option<Expr> = None;
        for (i, name) in names.iter().enumerate().rev() {
            let last = i == names.len() - 1;
            // A column has at least one character and doesn't start with a
            // separator; a `with` pattern runs at its start.
            let mut parts = vec![expr(ExprKind::Look {
                behind: false,
                negated: true,
                body: Box::new(stop.clone()),
            })];
            if let Some((_, _, with)) = self.spec.with.iter().find(|(n, _, _)| n == name) {
                parts.push(optional(with.clone()));
            }
            parts.push(expr(ExprKind::Until(Box::new(if last {
                text_end()
            } else {
                stop.clone()
            }))));
            let mut body = expr(ExprKind::Seq(parts));
            if let Some(&(_, _, ty)) = self.spec.types.iter().find(|(n, _, _)| n == name) {
                body = expr(ExprKind::Typed {
                    ty,
                    body: Box::new(body),
                });
            }
            let column = expr(ExprKind::Capture {
                name: name.clone(),
                body: Box::new(body),
            });
            row = Some(match row {
                None => column,
                Some(rest) => expr(ExprKind::Seq(vec![
                    column,
                    optional(expr(ExprKind::Seq(vec![separator.clone(), rest]))),
                ])),
            });
        }
        let row = row.expect("a header has columns");
        expr(ExprKind::Seq(vec![
            expr(ExprKind::Anchor(Anchor::LineStart)),
            optional(separator),
            row,
        ]))
    }
}

/// A header name as a capture name: lowercase, with runs of other characters
/// as `_` and none at the ends. `%CPU` is `cpu`, `Mounted on` is `mounted_on`.
fn capture_name(header: &str, index: usize) -> String {
    let mut name = String::new();
    let mut gap = false;
    for c in header.chars() {
        if c.is_alphanumeric() {
            if gap && !name.is_empty() {
                name.push('_');
            }
            gap = false;
            name.extend(c.to_lowercase());
        } else {
            gap = true;
        }
    }
    match name.as_str() {
        "" => format!("column{}", index + 1),
        "match" => "match_".to_owned(),
        n if n.starts_with(|c: char| c.is_ascii_digit()) => format!("_{n}"),
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_names() {
        let names: Vec<_> = ["%CPU", "USE%", "Mounted on", "NET I/O", "%", "MATCH", "1ST"]
            .iter()
            .enumerate()
            .map(|(i, h)| capture_name(h, i))
            .collect();
        assert_eq!(
            names,
            [
                "cpu",
                "use",
                "mounted_on",
                "net_i_o",
                "column5",
                "match_",
                "_1st"
            ]
        );
    }
}
