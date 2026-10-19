//! Source text to AST: lexer, parser, name resolution and the syntax tree.

mod ast;
mod lexer;
mod parser;
mod resolve;

pub(crate) use ast::{Anchor, Ast, Expr, ExprKind, Span};
pub(crate) use resolve::suggest_from;

pub(crate) fn parse(src: &str) -> Result<Ast, crate::error::Error> {
    resolve::resolve(src, parser::parse(src)?)
}
