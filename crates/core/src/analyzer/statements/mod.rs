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
use surrealdb::sql::{Kind, Statement, Value, statements::{DefineStatement, DefineTableStatement, DefineFieldStatement}, Permission, Permissions, Idiom};

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
            match define_stmt {
                DefineStatement::Function(func_def) => {
                    // Analyze function body to infer return type
                    let analyzed_func_def = crate::analyzer::functions::analyze_function_definition(ctx, func_def)?;
                    ctx.append_definition(DefineStatement::Function(analyzed_func_def));
                }
                DefineStatement::Table(table_def) => {
                    // Analyze table permissions and constraints
                    analyze_table_definition(ctx, table_def)?;
                    ctx.append_definition(define_stmt.clone());
                }
                DefineStatement::Field(field_def) => {
                    // Analyze field permissions, asserts, and defaults
                    analyze_field_definition(ctx, field_def)?;
                    ctx.append_definition(define_stmt.clone());
                }
                _ => {
                    // Other definition types - just store them
                    ctx.append_definition(define_stmt.clone());
                }
            }

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

/// Analyzes a table definition, extracting required parameters from permissions
fn analyze_table_definition(ctx: &mut AnalyzerContext, table_def: &DefineTableStatement) -> AnalyzerResult<()> {
    let table_name = &table_def.name.0;
    
    // Analyze table permissions
    analyze_permissions(ctx, &table_def.permissions, table_name, None)
        .map_err(|e| AnalyzerError::SchemaViolation {
            message: format!("Error in table '{}' permissions: {}", table_name, e),
            table: Some(table_name.clone()),
            field: None,
        })?;
    
    Ok(())
}

/// Analyzes expressions in VALUE, DEFAULT, and similar contexts where function calls should be properly evaluated
fn analyze_value_expression(ctx: &mut AnalyzerContext, value: &Value) -> AnalyzerResult<Kind> {
    match value {
        Value::Function(func) => {
            // In VALUE contexts, properly analyze function calls
            crate::analyzer::functions::analyze_function(ctx, func)
        },
        _ => {
            // For non-function values, use the standard resolve
            ctx.resolve(value)
        }
    }
}

/// Analyzes a field definition, extracting required parameters from permissions, asserts, and defaults
fn analyze_field_definition(ctx: &mut AnalyzerContext, field_def: &DefineFieldStatement) -> AnalyzerResult<()> {
    let table_name = &field_def.what.0;
    let field_name = field_def.name.to_string();
    
    // Analyze field permissions
    analyze_permissions(ctx, &field_def.permissions, table_name, Some(&field_name))
        .map_err(|e| AnalyzerError::SchemaViolation {
            message: format!("Error in field '{}.{}' permissions: {}", table_name, field_name, e),
            table: Some(table_name.clone()),
            field: Some(field_name.clone()),
        })?;
    
    // Analyze field assert clause
    if let Some(assert_value) = &field_def.assert {
        analyze_assert_clause(ctx, assert_value, table_name, &field_name, field_def)
            .map_err(|e| AnalyzerError::SchemaViolation {
                message: format!("Error in field '{}.{}' assert clause: {}", table_name, field_name, e),
                table: Some(table_name.clone()),
                field: Some(field_name.clone()),
            })?;
    }
    
    // Analyze field default value
    if let Some(default_value) = &field_def.value {
        analyze_default_value(ctx, default_value, table_name, &field_name, &field_def.kind)
            .map_err(|e| AnalyzerError::SchemaViolation {
                message: format!("Error in field '{}.{}' default value: {}", table_name, field_name, e),
                table: Some(table_name.clone()),
                field: Some(field_name.clone()),
            })?;
    }
    
    Ok(())
}



/// Analyzes permissions and extracts required parameters
fn analyze_permissions(ctx: &mut AnalyzerContext, permissions: &Permissions, table_name: &str, _field_name: Option<&str>) -> AnalyzerResult<()> {
    // Create a context with appropriate context variables for permission analysis
    let mut perm_ctx = ctx.clone();
    
    // Add context variables based on permission type
    // For now, add all possible context variables
    perm_ctx.add_context_variables_for_statement("SELECT", Some(table_name));
    perm_ctx.add_context_variables_for_statement("CREATE", Some(table_name));
    perm_ctx.add_context_variables_for_statement("UPDATE", Some(table_name));
    perm_ctx.add_context_variables_for_statement("DELETE", Some(table_name));
    
    // Analyze each permission clause
    if let Permission::Specific(select_perm) = &permissions.select {
        let result_type = perm_ctx.resolve(select_perm)?;
        validate_boolean_expression(&result_type, "SELECT permission")?;
    }
    
    if let Permission::Specific(create_perm) = &permissions.create {
        let result_type = perm_ctx.resolve(create_perm)?;
        validate_boolean_expression(&result_type, "CREATE permission")?;
    }
    
    if let Permission::Specific(update_perm) = &permissions.update {
        let result_type = perm_ctx.resolve(update_perm)?;
        validate_boolean_expression(&result_type, "UPDATE permission")?;
    }
    
    if let Permission::Specific(delete_perm) = &permissions.delete {
        let result_type = perm_ctx.resolve(delete_perm)?;
        validate_boolean_expression(&result_type, "DELETE permission")?;
    }
    
    // Extract any required parameters that were discovered
    let required_params = perm_ctx.get_all_required_params().to_vec();
    for (param_name, param_type) in required_params {
        ctx.add_required_param(&param_name, param_type);
    }
    
    Ok(())
}

/// Analyzes an assert clause and validates it returns a boolean
fn analyze_assert_clause(ctx: &mut AnalyzerContext, assert_value: &Value, table_name: &str, field_name: &str, field_def: &DefineFieldStatement) -> AnalyzerResult<()> {
    // Create context with $value representing the field being validated
    let mut assert_ctx = ctx.clone();
    
    // For field asserts, $value represents the field value
    // We already have the field definition from the function parameter
    if let Some(field_type) = &field_def.kind {
        assert_ctx.add_local_param("value", field_type.clone());
    }
    
    let result_type = analyze_value_expression(&mut assert_ctx, assert_value)?;
    validate_boolean_expression(&result_type, &format!("ASSERT clause for {}.{}", table_name, field_name))?;
    
    // Extract required parameters
    let required_params = assert_ctx.get_all_required_params().to_vec();
    for (param_name, param_type) in required_params {
        ctx.add_required_param(&param_name, param_type);
    }
    
    Ok(())
}

/// Analyzes a default value and validates it matches the field type
fn analyze_default_value(ctx: &mut AnalyzerContext, default_value: &Value, table_name: &str, _field_name: &str, field_type: &Option<Kind>) -> AnalyzerResult<()> {
    let mut default_ctx = ctx.clone();
    
    // Add context variables for default value evaluation
    default_ctx.add_context_variables_for_statement("CREATE", Some(table_name));
    
    // Add $value to context - it represents the input value for the field
    if let Some(expected_type) = field_type {
        default_ctx.add_local_param("value", expected_type.clone());
    } else {
        default_ctx.add_local_param("value", Kind::Any);
    }
    
    // Use proper expression analysis for VALUE contexts instead of simple resolve
    let default_type = analyze_value_expression(&mut default_ctx, default_value)?;
    
    // Validate that default value type is compatible with field type
    if let Some(expected_type) = field_type {
        if !is_type_compatible(&default_type, expected_type) {
            return Err(AnalyzerError::TypeMismatch {
                expected: format!("{:?}", expected_type),
                found: format!("{:?}", default_type),
            });
        }
    }
    
    // Extract required parameters
    let required_params = default_ctx.get_all_required_params().to_vec();
    for (param_name, param_type) in required_params {
        ctx.add_required_param(&param_name, param_type);
    }
    
    Ok(())
}

/// Validates that an expression returns a boolean type
fn validate_boolean_expression(result_type: &Kind, _context: &str) -> AnalyzerResult<()> {
    match result_type {
        Kind::Bool => Ok(()),
        Kind::Any => Ok(()), // Any could be boolean
        Kind::Either(types) => {
            // Union type - check if all variants are boolean-compatible
            if types.iter().all(|t| matches!(t, Kind::Bool | Kind::Any)) {
                Ok(())
            } else {
                Err(AnalyzerError::TypeMismatch {
                    expected: "boolean".to_string(),
                    found: format!("{:?}", result_type),
                })
            }
        }
        _ => Err(AnalyzerError::TypeMismatch {
            expected: "boolean".to_string(),
            found: format!("{:?}", result_type),
        })
    }
}

/// Checks if two types are compatible
fn is_type_compatible(actual: &Kind, expected: &Kind) -> bool {
    match (actual, expected) {
        // Any is compatible with everything
        (Kind::Any, _) | (_, Kind::Any) => true,
        // Exact match
        (a, b) if a == b => true,
        // String literals are compatible with string types
        (Kind::Literal(surrealdb::sql::Literal::String(_)), Kind::String) => true,
        // Number literals are compatible with number types
        (Kind::Literal(surrealdb::sql::Literal::Number(_)), Kind::Number) => true,
        // Bool literals are compatible with bool types
        (Kind::Literal(surrealdb::sql::Literal::Bool(_)), Kind::Bool) => true,
        // Arrays with compatible inner types
        (Kind::Array(actual_inner, _), Kind::Array(expected_inner, _)) => {
            is_type_compatible(actual_inner, expected_inner)
        }
        // Option types
        (Kind::Option(actual_inner), Kind::Option(expected_inner)) => {
            is_type_compatible(actual_inner, expected_inner)
        }
        // Non-option can be used where option is expected
        (actual_type, Kind::Option(expected_inner)) => {
            is_type_compatible(actual_type, expected_inner)
        }
        // Otherwise, not compatible
        _ => false,
    }
}
