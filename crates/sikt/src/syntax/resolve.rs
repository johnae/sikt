//! Turns a parsed pattern into an [`Ast`]: resolves rule names, inlines rules
//! that aren't recursive, turns references to recursive rules into calls,
//! rejects left recursion and lowers `nocase`.

use std::collections::HashMap;

use super::ast::{Ast, Expr, ExprKind, Span};
use super::parser::{Parsed, RESERVED, Rule, VOCABULARY};
use crate::class::Kind;
use crate::error::Error;

/// Keeps rules that use each other many times from expanding without bound.
const MAX_NODES: usize = 1 << 20;

pub(crate) fn resolve(src: &str, parsed: Parsed) -> Result<Ast, Error> {
    let Parsed { main, rules } = parsed;
    let mut index = HashMap::new();
    for (i, rule) in rules.iter().enumerate() {
        if index.insert(rule.name.as_str(), i).is_some() {
            return Err(Error::new(
                src,
                format!("rule `{}` is defined twice", rule.name),
                rule.span,
                None,
            )
            .with_help("rename one of them"));
        }
    }
    let mut refs = vec![Vec::new(); rules.len()];
    for (rule, refs) in rules.iter().zip(&mut refs) {
        check_names(src, &rule.body, &index, refs)?;
    }
    let mut main_refs = Vec::new();
    check_names(src, &main, &index, &mut main_refs)?;
    let unused_rules = rules
        .iter()
        .enumerate()
        .filter(|&(i, _)| !main_refs.iter().any(|&m| m == i || reaches(&refs, m, i)))
        .map(|(_, rule)| (rule.name.clone(), rule.span))
        .collect();

    let recursive: Vec<usize> = (0..rules.len()).filter(|&i| reaches(&refs, i, i)).collect();
    let mut r = Resolver {
        src,
        rules: &rules,
        index,
        call_index: recursive.iter().enumerate().map(|(k, &i)| (i, k)).collect(),
        inlined: HashMap::new(),
        nodes: 0,
    };
    let mut main = r.expand(&main)?;
    let mut bodies = recursive
        .iter()
        .map(|&i| r.expand(&rules[i].body))
        .collect::<Result<Vec<_>, _>>()?;

    // Nullability of recursive rules, as a fixpoint from "not nullable".
    let mut nullable = vec![false; bodies.len()];
    loop {
        let next: Vec<bool> = bodies.iter().map(|b| b.kind.nullable()).collect();
        if next == nullable {
            break;
        }
        nullable = next;
        for e in bodies.iter_mut().chain([&mut main]) {
            set_nullable(e, &nullable);
        }
    }

    for (k, &i) in recursive.iter().enumerate() {
        if left_calls(&bodies, k, &bodies[k], &mut vec![false; bodies.len()]) {
            let rule = &rules[i];
            let label = Some("calls itself before matching anything".into());
            return Err(Error::new(src, format!("rule `{}` is left-recursive", rule.name), rule.span, label)
                .with_help(r#"match something before the recursive call, like `let list = item 0+ ("," item)`"#));
        }
    }

    Ok(Ast {
        main: lower_nocase(main),
        rules: bodies.into_iter().map(lower_nocase).collect(),
        unused_rules,
    })
}

/// Checks that every word names a rule, collecting the rules referenced.
fn check_names(
    src: &str,
    e: &Expr,
    index: &HashMap<&str, usize>,
    refs: &mut Vec<usize>,
) -> Result<(), Error> {
    if let ExprKind::Rule(name) = &e.kind {
        match index.get(name.as_str()) {
            Some(&i) => refs.push(i),
            None => return Err(unknown_word(src, name, e.span, index.keys().copied())),
        }
    }
    e.kind
        .children()
        .iter()
        .try_for_each(|child| check_names(src, child, index, refs))
}

/// Whether rule `from` reaches rule `to` through references.
fn reaches(refs: &[Vec<usize>], from: usize, to: usize) -> bool {
    let mut seen = vec![false; refs.len()];
    let mut stack = refs[from].clone();
    while let Some(i) = stack.pop() {
        if i == to {
            return true;
        }
        if !std::mem::replace(&mut seen[i], true) {
            stack.extend(&refs[i]);
        }
    }
    false
}

struct Resolver<'a> {
    src: &'a str,
    rules: &'a [Rule],
    index: HashMap<&'a str, usize>,
    /// Rule index to call index, for recursive rules.
    call_index: HashMap<usize, usize>,
    /// Expanded bodies of rules that get inlined.
    inlined: HashMap<usize, Expr>,
    nodes: usize,
}

impl Resolver<'_> {
    fn expand(&mut self, e: &Expr) -> Result<Expr, Error> {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            let label = Some("expands to too many rule copies".into());
            return Err(Error::new(self.src, "pattern is too large", e.span, label));
        }
        let ExprKind::Rule(name) = &e.kind else {
            let mut out = e.clone();
            for child in out.kind.children_mut() {
                *child = self.expand(child)?;
            }
            return Ok(out);
        };
        let i = self.index[name.as_str()];
        if let Some(&rule) = self.call_index.get(&i) {
            let kind = ExprKind::Call {
                rule,
                nullable: false,
                nocase: false,
            };
            return Ok(Expr::new(kind, e.span));
        }
        let body = match self.inlined.get(&i) {
            Some(body) => body.clone(),
            None => {
                let body = self.expand(&self.rules[i].body)?;
                self.inlined.insert(i, body.clone());
                body
            }
        };
        self.nodes += count(&body);
        Ok(body)
    }
}

fn count(e: &Expr) -> usize {
    1 + e.kind.children().iter().map(count).sum::<usize>()
}

fn set_nullable(e: &mut Expr, table: &[bool]) {
    if let ExprKind::Call { rule, nullable, .. } = &mut e.kind {
        *nullable = table[*rule];
    }
    for child in e.kind.children_mut() {
        set_nullable(child, table);
    }
}

/// Whether `e` can call rule `target` before consuming input.
fn left_calls(rules: &[Expr], target: usize, e: &Expr, seen: &mut [bool]) -> bool {
    match &e.kind {
        ExprKind::Call { rule, .. } => {
            *rule == target
                || (!std::mem::replace(&mut seen[*rule], true)
                    && left_calls(rules, target, &rules[*rule], seen))
        }
        ExprKind::Seq(items) => {
            for item in items {
                if left_calls(rules, target, item, seen) {
                    return true;
                }
                if !item.kind.nullable() {
                    return false;
                }
            }
            false
        }
        kind => kind
            .children()
            .iter()
            .any(|child| left_calls(rules, target, child, seen)),
    }
}

fn lower_nocase(mut e: Expr) -> Expr {
    for child in e.kind.children_mut() {
        let placeholder = Expr::new(ExprKind::Any, child.span);
        *child = lower_nocase(std::mem::replace(child, placeholder));
    }
    match e.kind {
        ExprKind::NoCase(body) => body.fold_case(),
        _ => e,
    }
}

fn unknown_word<'a>(
    src: &str,
    word: &str,
    span: Span,
    rules: impl Iterator<Item = &'a str>,
) -> Error {
    let label = suggest(word, rules).map(|s| format!("did you mean `{s}`?"));
    Error::new(src, format!("unknown word `{word}`"), span, label)
        .with_help(format!("to match the text, quote it: \"{word}\""))
}

/// The closest known word or rule name within a small edit distance.
fn suggest<'a>(word: &str, rules: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let kinds = Kind::ALL.iter().map(|k| k.name());
    VOCABULARY
        .iter()
        .copied()
        .chain(kinds)
        .chain(rules)
        .filter(|candidate| !RESERVED.contains(candidate))
        .map(|candidate| (edit_distance(word, candidate), candidate))
        .filter(|&(d, _)| d <= 2 && d < word.chars().count())
        .min_by_key(|&(d, _)| d)
        .map(|(_, candidate)| candidate)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            let next = (row[j + 1] + 1).min(row[j] + 1).min(prev + cost);
            prev = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions() {
        let none = || std::iter::empty();
        assert_eq!(suggest("digts", none()), Some("digit"));
        assert_eq!(suggest("leter", none()), Some("letter"));
        assert_eq!(suggest("xyzzy", none()), None);
        assert_eq!(suggest("a", none()), None);
        assert_eq!(suggest("octt", ["octet"].into_iter()), Some("octet"));
    }
}
