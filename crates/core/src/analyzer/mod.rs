// analyzer/mod.rs
//! Static analysis system for SurrealQL queries.
//!
//! The analyzer performs type checking, schema validation, and permissions analysis
//! on SurrealQL queries before they are executed. This helps catch errors early and
//! ensures queries conform to the database schema and access rules.
//!
//! # Architecture
//!
//! The analyzer is divided into several key components:
//!
//! - [`context`]: Maintains analysis state including schema info, parameters, and functions
//! - [`statements`]: Statement-specific analyzers for different query types (SELECT, CREATE, etc)
//! - [`error`]: Error types specific to analysis failures
//! - [`functions`]: Analysis of built-in and custom functions

pub mod context;
pub mod statements;
pub mod error;
pub mod functions;

use context::AnalyzerContext;
use error::{AnalyzerError, AnalyzerResult};
use surrealdb::sql::{Kind, Value};

/// Analyzes a SurrealQL query string and returns the types of all statements.
///
/// # Arguments
/// * `ctx` - Analysis context containing schema and other metadata
/// * `surql` - The SurrealQL query string to analyze
///
/// # Returns
/// A vector containing the result type of each statement in the query,
/// or an error if analysis fails.
///
/// # Errors
/// Returns an AnalyzerError if:
/// - The query fails to parse
/// - Any statement violates schema constraints
/// - Type checking fails
/// - Referenced tables/fields don't exist
pub fn analyze(ctx: &mut AnalyzerContext, surql: &str) -> AnalyzerResult<Kind> {
    // Parse the query string into AST
    let statements = surrealdb::sql::parse(surql)
        .map_err(AnalyzerError::Surreal)?;

    // Analyze each statement
    let kinds: Vec<Kind> = statements.iter()
        .map(|stmt| statements::analyze_statement(ctx, stmt))
        .collect::<Result<Vec<_>, _>>()?;

    match kinds.len() {
        0 => Ok(Kind::Null),
        1 => Ok(kinds[0].clone()),
        _ => Ok(Kind::Either(kinds))
    }
}
