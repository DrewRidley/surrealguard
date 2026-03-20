//! Statement analyzers — one module per statement type.
//!
//! Each module exports an `analyze` function with the signature:
//!   fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind
//!
//! Statement modules are self-contained. They handle their own clause
//! processing, field validation, and result type computation.

pub mod select;
pub mod create;
pub mod update;
pub mod delete;
pub mod insert;
pub mod upsert;
pub mod relate;
pub mod let_stmt;
