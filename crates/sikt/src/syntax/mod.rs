//! Source text to AST: lexer, parser and the syntax tree.

mod ast;
mod lexer;
mod parser;

pub(crate) use ast::{Expr, ExprKind, Span};
pub(crate) use parser::parse;
