//! Source text to AST: lexer, parser, name resolution and the syntax tree.

mod ast;
mod lexer;
mod parser;
mod resolve;

use std::fmt;
use std::sync::Arc;

pub(crate) use ast::{
    Anchor, Ast, CmpOp, Cond, Expr, ExprKind, Span, Statements, TableSpec, Test, Unit,
};
pub(crate) use resolve::suggest_from;

use crate::error::Error;
use parser::Import;

/// Built-in modules, by name.
const MODULES: &[(&str, &str)] = &[
    ("id", include_str!("../modules/id.sikt")),
    ("net", include_str!("../modules/net.sikt")),
    ("text", include_str!("../modules/text.sikt")),
    ("time", include_str!("../modules/time.sikt")),
];

/// Reads a file by the path a `use` gives, or says why it can't.
pub(crate) type Load = dyn Fn(&str) -> Result<String, String> + Send + Sync;

/// Reads the file a `use "path"` names.
#[derive(Clone)]
pub(crate) struct Loader(pub Arc<Load>);

impl fmt::Debug for Loader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Loader")
    }
}

/// Parses and resolves `src` with the modules it uses. Returns the source the
/// spans point into: `src`, followed by the text of each module.
pub(crate) fn parse(src: &str, loader: Option<&Loader>) -> Result<(Ast, String), Error> {
    let mut parsed = parser::parse(src)?;
    let mut text = src.to_owned();
    let mut pending = std::mem::take(&mut parsed.uses);
    let mut loaded: Vec<Import> = Vec::new();
    // Each module: its name, where its text starts, and the text.
    let mut modules: Vec<(String, usize, String)> = Vec::new();
    while let Some((import, span)) = pending.pop() {
        if loaded.contains(&import) {
            continue;
        }
        let (name, module_text) = match &import {
            Import::Module(name) => {
                let Some((_, module)) = MODULES.iter().find(|(n, _)| n == name) else {
                    let names: Vec<&str> = MODULES.iter().map(|(n, _)| *n).collect();
                    let label = suggest_from(name, names.iter().copied())
                        .map(|s| format!("did you mean `{s}`?"));
                    return Err(
                        Error::new(&text, format!("unknown module `{name}`"), span, label)
                            .with_help(format!("the modules are {}", names.join(", "))),
                    );
                };
                (name.clone(), (*module).to_owned())
            }
            Import::File(path) => {
                let Some(Loader(load)) = loader else {
                    return Err(Error::new(&text, "importing files needs a loader", span, None)
                        .with_help("the `sikt` command reads files; in the library, set one with `PatternBuilder::import_with`"));
                };
                let module = load(path).map_err(|e| {
                    Error::new(&text, format!("can't read `{path}`: {e}"), span, None)
                })?;
                (path.clone(), module)
            }
        };
        let module =
            parser::parse_module(&module_text).map_err(|e| e.in_file(&name, &module_text))?;
        // The module's text follows what came before, so its spans move by
        // where it starts.
        text.push_str(&format!("\n\n# use {name}\n"));
        let offset = text.len();
        text.push_str(&module_text);
        for mut rule in module.rules {
            rule.span = rule.span.shifted(offset);
            rule.body.shift(offset);
            rule.imported = true;
            parsed.rules.push(rule);
        }
        pending.extend(
            module
                .uses
                .into_iter()
                .map(|(import, span)| (import, span.shifted(offset))),
        );
        modules.push((name, offset, module_text));
        loaded.push(import);
    }
    let ast = resolve::resolve(&text, parsed).map_err(|e| {
        // An error in a module shows that module's file.
        let module = modules
            .iter()
            .rev()
            .find(|(_, offset, _)| e.span().start >= *offset);
        match module {
            Some((name, offset, module)) => e.moved_to(name, module, *offset),
            None => e,
        }
    })?;
    Ok((ast, text))
}
