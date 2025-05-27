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
// pub(crate) mod logic;
// pub(crate) mod system;

use crate::analyzer::{
    context::AnalyzerContext,
    error::{AnalyzerError, AnalyzerResult},
};
use surrealdb::sql::{Kind, Statement, Value};

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
        Statement::Value(value) => match value {
            Value::Function(func) => {
                // Handle function calls properly instead of using ctx.resolve
                crate::analyzer::functions::analyze_function(ctx, func)
            },
            _ => ctx.resolve(value),
        },
        // Data query statements
        Statement::Select(select_stmt) => self::data::analyze_select(ctx, select_stmt),
        Statement::Update(update_stmt) => self::data::analyze_update(ctx, update_stmt),
        Statement::Create(create_stmt) => self::data::analyze_create(ctx, create_stmt),
        Statement::Delete(delete_stmt) => self::data::analyze_delete(ctx, delete_stmt),
        Statement::Insert(insert_stmt) => self::data::analyze_insert(ctx, insert_stmt),
        Statement::Upsert(upsert_stmt) => self::data::analyze_upsert(ctx, upsert_stmt),
        Statement::Relate(relate_stmt) => self::data::analyze_relate(ctx, relate_stmt),

        // Control flow statements
        Statement::Foreach(_foreach_stmt) => {
            // FOREACH statements don't return values, they execute their block for each iteration
            Ok(Kind::Null)
        }

        // Variable assignment statements
        Statement::Set(_set_stmt) => {
            // SET statements define variables and don't return values
            Ok(Kind::Null)
        }

        // Transaction control statements
        Statement::Begin(_begin_stmt) => {
            // BEGIN statements start transactions and don't return values
            Ok(Kind::Null)
        }
        Statement::Commit(_commit_stmt) => {
            // COMMIT statements end transactions and don't return values
            Ok(Kind::Null)
        }
        Statement::Cancel(_cancel_stmt) => {
            // CANCEL statements abort transactions and don't return values
            Ok(Kind::Null)
        }

        // Schema definition statements
        Statement::Define(define_stmt) => {
            ctx.append_definition(define_stmt.clone());

            //A define statement returns nothing.
            Ok(Kind::Null)
        }

        // Schema removal statements
        Statement::Remove(remove_stmt) => {
            use surrealdb::sql::statements::RemoveStatement;
            match remove_stmt {
                RemoveStatement::Table(remove_table) => {
                    let table_name = &remove_table.name.0;
                    if ctx.find_table_definition(table_name).is_some() {
                        ctx.remove_table_definition(table_name);
                        Ok(Kind::Null)
                    } else if remove_table.if_exists {
                        // IF EXISTS clause means no error if table doesn't exist
                        Ok(Kind::Null)
                    } else {
                        Err(AnalyzerError::TableNotFound(table_name.clone()))
                    }
                }
                RemoveStatement::Field(remove_field) => {
                    let table_name = &remove_field.what.0;
                    let field_name = &remove_field.name;
                    if ctx.find_field_definition(table_name, field_name).is_some() {
                        ctx.remove_field_definition(table_name, field_name);
                        Ok(Kind::Null)
                    } else if remove_field.if_exists {
                        // IF EXISTS clause means no error if field doesn't exist
                        Ok(Kind::Null)
                    } else {
                        Err(AnalyzerError::field_not_found(
                            field_name.to_string(),
                            format!("table {}", table_name)
                        ))
                    }
                }
                RemoveStatement::Index(_remove_index) => {
                    // Index removal doesn't affect type analysis, just return null
                    Ok(Kind::Null)
                }
                _ => {
                    // Other REMOVE types not implemented yet
                    Err(AnalyzerError::Unimplemented(
                        format!("REMOVE statement type not implemented: {:?}", remove_stmt)
                    ))
                }
            }
        }

        // Schema alteration statements
        Statement::Alter(alter_stmt) => {
            use surrealdb::sql::statements::AlterStatement;
            match alter_stmt {
                AlterStatement::Table(alter_table) => {
                    let table_name = &alter_table.name.0;
                    
                    // Check if table exists (unless IF EXISTS is used)
                    if ctx.find_table_definition(table_name).is_none() {
                        if alter_table.if_exists {
                            // IF EXISTS clause means no error if table doesn't exist
                            return Ok(Kind::Null);
                        } else {
                            return Err(AnalyzerError::TableNotFound(table_name.clone()));
                        }
                    }

                    // Apply the alterations to the existing table definition
                    ctx.alter_table_definition(alter_table)?;
                    Ok(Kind::Null)
                }
                _ => {
                    // Other ALTER types not implemented yet
                    Err(AnalyzerError::Unimplemented(
                        format!("ALTER statement type not implemented: {:?}", alter_stmt)
                    ))
                }
            }
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
