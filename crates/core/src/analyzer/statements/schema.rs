//! Analysis of schema definition statements.
//!
//! This module contains analyzers for SurrealQL schema definition statements:
//!
//! - DEFINE TABLE
//! - DEFINE FIELD
//! - DEFINE INDEX
//! - DEFINE EVENT
//! - DEFINE FUNCTION
//! - DEFINE PARAM
//! - DEFINE SCOPE
//! - DEFINE TOKEN
//! - DEFINE ANALYZER
//! - DEFINE DATABASE
//! - DEFINE NAMESPACE
//!
//! Each statement type has specialized analysis logic that validates
//! the statement against schema rules and updates the analyzer context.

use crate::analyzer::{
    context::AnalyzerContext,
    error::{AnalyzerError, AnalyzerResult},
};
use surrealdb::sql::{
    statements::{
        DefineAnalyzerStatement, DefineDatabaseStatement, DefineEventStatement, DefineFieldStatement,
        DefineFunctionStatement, DefineIndexStatement, DefineNamespaceStatement, DefineParamStatement,
        DefineScopeStatement, DefineStatement, DefineTableStatement, DefineTokenStatement,
    },
    Kind,
};

/// Analyzes a DEFINE statement.
///
/// Routes the statement to the appropriate analyzer based on its type
/// and returns the resulting type or error.
///
/// # Arguments
/// * `ctx` - The analysis context
/// * `stmt` - The DEFINE statement to analyze
///
/// # Returns
/// The type produced by the statement, or an error if analysis fails.
pub fn analyze_define(ctx: &mut AnalyzerContext, stmt: &DefineStatement) -> AnalyzerResult<Kind> {
    match stmt {
        DefineStatement::Table(table_stmt) => analyze_define_table(ctx, table_stmt),
        DefineStatement::Field(field_stmt) => analyze_define_field(ctx, field_stmt),
        DefineStatement::Index(index_stmt) => analyze_define_index(ctx, index_stmt),
        DefineStatement::Event(event_stmt) => analyze_define_event(ctx, event_stmt),
        DefineStatement::Function(function_stmt) => analyze_define_function(ctx, function_stmt),
        DefineStatement::Param(param_stmt) => analyze_define_param(ctx, param_stmt),
        DefineStatement::Scope(scope_stmt) => analyze_define_scope(ctx, scope_stmt),
        DefineStatement::Token(token_stmt) => analyze_define_token(ctx, token_stmt),
        DefineStatement::Analyzer(analyzer_stmt) => analyze_define_analyzer(ctx, analyzer_stmt),
        DefineStatement::Database(database_stmt) => analyze_define_database(ctx, database_stmt),
        DefineStatement::Namespace(namespace_stmt) => analyze_define_namespace(ctx, namespace_stmt),
    }
}

/// Analyzes a DEFINE TABLE statement.
fn analyze_define_table(
    ctx: &mut AnalyzerContext,
    stmt: &DefineTableStatement,
) -> AnalyzerResult<Kind> {
    // Validate table name
    if stmt.name.0.is_empty() {
        return Err(AnalyzerError::schema_violation(
            "Table name cannot be empty",
            None,
            None,
        ));
    }

    // Store the table definition in the context
    ctx.append_definition(DefineStatement::Table(stmt.clone()));

    // A define statement returns nothing
    Ok(Kind::Null)
}

/// Analyzes a DEFINE FIELD statement.
fn analyze_define_field(
    ctx: &mut AnalyzerContext,
    stmt: &DefineFieldStatement,
) -> AnalyzerResult<Kind> {
    // Validate table exists
    if ctx.find_table_definition(&stmt.what.0).is_none() {
        return Err(AnalyzerError::table_not_found(stmt.what.0.clone()));
    }

    // Validate field name
    if stmt.name.0.is_empty() {
        return Err(AnalyzerError::schema_violation(
            "Field name cannot be empty",
            Some(&stmt.what.0),
            None,
        ));
    }

    // Store the field definition in the context
    ctx.append_definition(DefineStatement::Field(stmt.clone()));

    // A define statement returns nothing
    Ok(Kind::Null)
}

/// Analyzes a DEFINE INDEX statement.
fn analyze_define_index(
    ctx: &mut AnalyzerContext,
    stmt: &DefineIndexStatement,
) -> AnalyzerResult<Kind> {
    // Validate table exists
    if ctx.find_table_definition(&stmt.what.0).is_none() {
        return Err(AnalyzerError::TableNotFound(stmt.what.0.clone()));
    }

    // Validate index name
    if stmt.name.is_empty() {
        return Err(AnalyzerError::schema_violation(
            "Index name cannot be empty",
            Some(&stmt.what.0),
            None::<String>,
        ));
    }

    // Store the index definition in the context
    ctx.append_definition(DefineStatement::Index(stmt.clone()));

    // A define statement returns nothing
    Ok(Kind::Null)
}