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
pub mod error;
pub mod functions;
pub mod statements;

use context::AnalyzerContext;
use error::{AnalyzerError, AnalyzerResult};
use surrealdb::sql::{Kind, Literal};

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
    let statements = surrealdb::sql::parse(surql).map_err(AnalyzerError::Surreal)?;

    // Analyze each statement
    let kinds: Vec<Kind> = statements
        .iter()
        .map(|stmt| statements::analyze_statement(ctx, stmt))
        .collect::<Result<Vec<_>, _>>()?;



    match kinds.len() {
        0 => Ok(Kind::Null),
        //TODO: consolidate this after refactoring the kind! macro.
        1 => Ok(Kind::Array(Box::new(kinds[0].clone()), None)),
        _ => Ok(Kind::Literal(Literal::Array(kinds))),
    }
}

#[cfg(test)]
mod test {
    use surrealguard_macros::kind;

    use crate::analyzer::{analyze, context::AnalyzerContext};

    #[test]
    fn multiple_statements() {
        let mut ctx = AnalyzerContext::new();

        // Build schema
        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE organization SCHEMAFULL;
                DEFINE FIELD name ON organization TYPE string;
                DEFINE FIELD desc ON organization TYPE string;
                DEFINE FIELD industry ON organization TYPE string;

            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD email ON user TYPE string;
                DEFINE FIELD password ON user TYPE string;
                DEFINE FIELD name ON user TYPE string;
                DEFINE FIELD organization ON user TYPE record<organization>;
        "#,
        )
        .expect("Schema construction should succeed");

        // Test multi-statement query
        let stmt = r#"
            CREATE organization:applebees CONTENT {
                name: "AppleBees",
                desc: "A big restaurant",
                industry: "food"
            };

            CREATE user:jane CONTENT {
                email: "jane@doe.org",
                password: crypto::argon2::generate("password"),
                name: "Jane Doe",
                organization: organization:applebees
            };
        "#;

        let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");

        // Should be a literal array with exact types for each statement
        let expected_kind = kind!(
            r#"[
                array<{ name: string, desc: string, industry: string }>,
                array<{ email: string, password: string, name: string, organization: record<organization> }>
            ]"#
        );

        assert_eq!(analyzed_kind, expected_kind);
    }
}
