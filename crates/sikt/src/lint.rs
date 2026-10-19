//! Warnings about patterns that compile but probably don't do what was meant.

use crate::class::CharSet;
use crate::error::Warning;
use crate::syntax::{Ast, Expr, ExprKind};

/// In `line_mode` the pattern runs on single lines, so `newline` can't match.
pub(crate) fn lint(ast: &Ast, src: &str, line_mode: bool) -> Vec<Warning> {
    let mut linter = Linter {
        src,
        line_mode,
        warnings: Vec::new(),
    };
    linter.expr(&ast.main);
    ast.rules.iter().for_each(|rule| linter.expr(rule));
    for (name, span) in &ast.unused_rules {
        let help = "remove it, or use it in the pattern";
        linter.warnings.push(Warning::new(
            src,
            format!("rule `{name}` is never used"),
            *span,
            help,
        ));
    }
    // A rule used in several places is inlined in each, so its warnings repeat.
    let mut seen = Vec::new();
    linter.warnings.retain(|w| {
        let key = (w.message().to_owned(), w.span());
        if seen.contains(&key) {
            return false;
        }
        seen.push(key);
        true
    });
    linter.warnings
}

struct Linter<'s> {
    src: &'s str,
    line_mode: bool,
    warnings: Vec<Warning>,
}

impl Linter<'_> {
    fn text(&self, e: &Expr) -> &str {
        &self.src[e.span.start..e.span.end]
    }

    fn warn(&mut self, message: String, e: &Expr, help: impl Into<String>) {
        self.warnings
            .push(Warning::new(self.src, message, e.span, help));
    }

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Choice(alts) => {
                self.unreachable_options(alts);
                self.until_in_choice(alts);
            }
            ExprKind::Seq(items) => self.possessive_traps(items),
            ExprKind::Newline if self.line_mode => {
                let message =
                    "`newline` never matches in line mode, where lines have no line break";
                self.warn(
                    message.into(),
                    e,
                    "use `line end`, or match across lines with -U",
                );
            }
            ExprKind::Until(x) | ExprKind::Through(x) if never_fails(x) => {
                let keyword = if matches!(e.kind, ExprKind::Until(_)) {
                    "until"
                } else {
                    "through"
                };
                let message = format!(
                    "`{keyword}` stops at once because `{}` always matches",
                    self.text(x)
                );
                self.warn(
                    message,
                    x,
                    "make what it stops at match at least one character",
                );
            }
            ExprKind::Until(x) if has_capture(x) => {
                let message = format!(
                    "`until` stops before `{}`, so the captures in it are never set",
                    self.text(x)
                );
                self.warn(
                    message,
                    x,
                    "use `through` to match it and keep its captures",
                );
            }
            _ => {}
        }
        e.kind.children().iter().for_each(|child| self.expr(child));
    }

    /// An option that an earlier one always beats: `"a" or "ab"`.
    fn unreachable_options(&mut self, alts: &[Expr]) {
        for (j, later) in alts.iter().enumerate().skip(1) {
            let later_prefix = later.kind.literal_prefix().0;
            for earlier in &alts[..j] {
                let (prefix, complete) = earlier.kind.literal_prefix();
                let (reason, help) = if never_fails(earlier) {
                    (
                        "always matches",
                        "move the option that can match empty last",
                    )
                } else if complete && !prefix.is_empty() && later_prefix.starts_with(&prefix) {
                    ("matches first", "put the longer option first")
                } else {
                    continue;
                };
                let message = format!(
                    "`{}` can never match because `{}` {reason}",
                    self.text(later),
                    self.text(earlier)
                );
                self.warn(message, later, help);
                break;
            }
        }
    }

    /// `until "a" or "b"`, which reads as stopping at either but is
    /// `(until "a") or "b"`.
    fn until_in_choice(&mut self, alts: &[Expr]) {
        for (i, alt) in alts.iter().enumerate().take(alts.len() - 1) {
            let last = match &alt.kind {
                ExprKind::Seq(items) => &items[items.len() - 1],
                _ => alt,
            };
            let (ExprKind::Until(x) | ExprKind::Through(x)) = &last.kind else {
                continue;
            };
            // A parenthesized `(until x)` was meant.
            let keyword = if self.text(last).starts_with("until") {
                "until"
            } else if self.text(last).starts_with("through") {
                "through"
            } else {
                continue;
            };
            let options: Vec<&str> = std::iter::once(self.text(x))
                .chain(alts[i + 1..].iter().map(|a| self.text(a)))
                .collect();
            let message = format!(
                "`{keyword}` applies only to `{}` here, not to the whole `or`",
                self.text(x)
            );
            let help = format!(
                "to stop at any of the options, write `{keyword} ({})`",
                options.join(" or ")
            );
            self.warn(message, last, help);
        }
    }

    /// A possessive repetition that takes everything the next item needs:
    /// `0+ any "x"` or `1+ letter "s"`.
    fn possessive_traps(&mut self, items: &[Expr]) {
        for pair in items.windows(2) {
            let (rep, next) = (&pair[0], &pair[1]);
            let ExprKind::Repeat {
                max: None, body, ..
            } = &rep.kind
            else {
                continue;
            };
            if next.kind.nullable() {
                continue;
            }
            let (taken, help) = match &body.kind {
                ExprKind::Any => (
                    "everything".to_owned(),
                    format!("use `until {}` to match up to it", self.text(next)),
                ),
                ExprKind::Class(set) => {
                    let possessive =
                        "repetition is possessive: it never gives characters back".to_owned();
                    match next.kind.literal_prefix().0.chars().next() {
                        Some(c) if set.matches(c) => {
                            (format!("every `{:?}`", c.to_string()), possessive)
                        }
                        Some(_) => continue,
                        None => match first_class(next) {
                            Some((class, inner)) if inner.is_subset_of(set) => {
                                (format!("every `{}`", self.text(class)), possessive)
                            }
                            _ => continue,
                        },
                    }
                }
                _ => continue,
            };
            let message = format!(
                "`{}` takes {taken}, so `{}` can never match after it",
                self.text(rep),
                self.text(next)
            );
            self.warn(message, next, help);
        }
    }
}

/// Whether `e` captures anything.
fn has_capture(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::Capture { .. }) || e.kind.children().iter().any(has_capture)
}

/// Whether `e` succeeds at every position, like `optional x` or `0+ x`.
fn never_fails(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Literal(s) => s.is_empty(),
        ExprKind::Repeat { min, body, .. } => *min == 0 || never_fails(body),
        ExprKind::Seq(items) => items.iter().all(never_fails),
        ExprKind::Choice(alts) => alts.iter().any(never_fails),
        ExprKind::Capture { body, .. } | ExprKind::Typed { body, .. } => never_fails(body),
        _ => false,
    }
}

/// The character set an expression starts with, through sequences and
/// captures, with the expression that holds it.
fn first_class(e: &Expr) -> Option<(&Expr, &CharSet)> {
    match &e.kind {
        ExprKind::Class(set) => Some((e, set)),
        ExprKind::Seq(items) => first_class(&items[0]),
        ExprKind::Capture { body, .. } | ExprKind::Typed { body, .. } => first_class(body),
        ExprKind::Repeat { min, body, .. } if *min >= 1 => first_class(body),
        _ => None,
    }
}
