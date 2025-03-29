//! Analysis of individual SurrealQL statement types.
//!
//! This module contains analyzers for different categories of SurrealQL statements:
//!
//! - Data manipulation (SELECT, CREATE, UPDATE, DELETE)
//! - Schema definition (DEFINE TABLE, DEFINE FIELD)
//! - System commands (INFO, USE)
//!
//! Each statement type has its own submodule with specialized analysis logic
//! that validates the statement against schema rules and determines result types.

pub(crate) mod data;
pub(crate) mod schema;
// pub(crate) mod logic;
// pub(crate) mod system;

use crate::analyzer::{
    context::AnalyzerContext,
    error::{AnalyzerError, AnalyzerResult},
};
use surrealdb::sql::{Kind, Statement};

/// Analyzes a single SurrealQL statement.
///
/// Routes the statement to the appropriate analyzer based on its type
/// and returns the resulting type or error.
///
/// # Arguments
/// * `ctx` - The analysis context
/// * `stmt` - The statement to analyze
///
/// # Returns
/// The type produced by the statement, or an error if analysis fails.
pub fn analyze_statement(ctx: &mut AnalyzerContext, stmt: &Statement) -> AnalyzerResult<Kind> {
    match stmt {
        // Direct value statement (e.g., "SELECT 1 + 1")
        Statement::Value(value) => ctx.resolve(value),
        // Data query statements
        Statement::Select(select_stmt) => self::data::analyze_select(ctx, select_stmt),
        Statement::Update(update_stmt) => self::data::analyze_update(ctx, update_stmt),
        Statement::Create(create_stmt) => self::data::analyze_create(ctx, create_stmt),
        Statement::Delete(delete_stmt) => self::data::analyze_delete(ctx, delete_stmt),
        Statement::Insert(insert_stmt) => self::data::analyze_insert(ctx, insert_stmt),
        Statement::Upsert(upsert_stmt) => self::data::analyze_upsert(ctx, upsert_stmt),
        Statement::Relate(relate_stmt) => self::data::analyze_relate(ctx, relate_stmt),

        // Schema definition statements
        Statement::Define(define_stmt) => {
            // First analyze the DEFINE statement
            let result = self::schema::analyze_define(ctx, define_stmt);
            
            // If analysis fails, return the error
            if result.is_err() {
                return result;
            }
            
            // A define statement returns nothing
            Ok(Kind::Null)
        }
        // Other statement types
        _ => Err(AnalyzerError::Surreal(
            surrealdb::err::Error::Unimplemented(format!(
                "Analysis not implemented for {:?}",
                stmt
            )),
        )),
    }
}