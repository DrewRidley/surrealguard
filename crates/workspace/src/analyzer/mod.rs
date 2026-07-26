//! The SurrealQL analyzer tree: one module per statement, expression, or
//! function family, each owning its own analysis logic.
//!
//! Analyzers consume the typed AST from `surrealguard_syntax::ast` (lowered
//! once per source by `surrealguard_syntax::lower`) and infer upstream
//! `surrealdb_types::Kind` response types: closed objects are
//! `Kind::Literal(KindLiteral::Object(..))`, and undeterminable positions
//! are `Kind::Any` poison values. Diagnostics are appended through the
//! shared [`context::AnalysisContext`]; per-statement invariants belong to
//! the analyzer that owns their statement.
//!
//! [`pipeline`] is the entry point: it walks each source in statement
//! order, dispatching every lowered statement to its analyzer against the
//! schema built so far.

pub mod const_eval;
pub mod context;
pub mod data;
pub mod expression;
pub(crate) mod facts;
pub mod flow;
pub mod function;
pub mod pipeline;
pub mod schema;
pub mod statement;
pub mod system;

#[cfg(test)]
pub(crate) mod test_support;
