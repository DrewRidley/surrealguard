//! SurrealQL syntax: parsing (tree-sitter), the typed span-carrying AST,
//! and the lowering pass between them. This crate owns everything that
//! reads source text; analysis consumes only `ast::*` values.

pub mod ast;
pub mod highlight;
pub mod lower;
pub mod parse;
pub mod source;
pub mod span;
