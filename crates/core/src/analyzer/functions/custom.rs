use surrealdb::sql::{statements::DefineFunctionStatement, Block, Entry, Kind};
use crate::analyzer::context::AnalyzerContext;
use crate::analyzer::error::{AnalyzerError, AnalyzerResult};

/// Analyzes a custom function definition and infers its return type from the body
pub fn analyze_function_definition(ctx: &mut AnalyzerContext, func_def: &DefineFunctionStatement) -> AnalyzerResult<DefineFunctionStatement> {
    // Analyze function body to get return type and capture required parameters
    let (inferred_return_type, required_params) = analyze_function_body(ctx, func_def)?;
    
    // Store required parameters as metadata for this function
    let function_name = format!("fn::{}", func_def.name.0);
    ctx.set_function_required_params(&function_name, required_params);
    
    let final_return_type = match &func_def.returns {
        Some(declared_type) => {
            // Validate that declared type is compatible with inferred type
            if is_compatible_type(declared_type, &inferred_return_type) {
                declared_type.clone()
            } else {
                return Err(AnalyzerError::FunctionReturnTypeMismatch {
                    declared: declared_type.clone(),
                    inferred: inferred_return_type,
                });
            }
        }
        None => {
            // No declared type, use inferred type
            inferred_return_type
        }
    };

    // Create new function definition with the final return type
    let mut analyzed_func_def = func_def.clone();
    analyzed_func_def.returns = Some(final_return_type);
    
    Ok(analyzed_func_def)
}

/// Analyzes a function body to infer its return type and capture required parameters
fn analyze_function_body(ctx: &AnalyzerContext, func_def: &DefineFunctionStatement) -> AnalyzerResult<(Kind, Vec<(String, Kind)>)> {
    // Create function context with parameters
    let function_params: Vec<(String, Kind)> = func_def.args.iter()
        .map(|(name, kind)| (name.0.clone(), kind.clone()))
        .collect();
    let mut func_ctx = ctx.with_function_params(&function_params);
    
    // Analyze the function body to get return type
    let return_type = analyze_block(&mut func_ctx, &func_def.block)?;
    
    // Extract required parameters that were discovered during analysis
    let required_params = func_ctx.get_all_required_params().to_vec();
    
    Ok((return_type, required_params))
}

/// Analyzes a block of statements and infers the return type
fn analyze_block(ctx: &mut AnalyzerContext, block: &Block) -> AnalyzerResult<Kind> {
    let mut return_types = Vec::new();
    let mut has_explicit_return = false;

    for entry in &block.0 {
        match entry {
            Entry::Output(output) => {
                // RETURN statement - analyze the returned value
                let return_type = ctx.resolve(&output.what)?;
                return_types.push(return_type);
                has_explicit_return = true;
            }
            Entry::Set(set_stmt) => {
                // Check if this is a cast operation: <type> $variable;
                if let Some((var_name, cast_type)) = extract_cast_operation(set_stmt) {
                    // Cast operation - override the variable type with higher precedence
                    ctx.add_local_param(&var_name, cast_type);
                } else if let Some(var_name) = extract_let_variable(set_stmt) {
                    // LET variable = value - add to local context
                    let value_type = ctx.resolve(&set_stmt.what)?;
                    ctx.add_local_param(&var_name, value_type);
                } else {
                    // This is a SET operation - analyze it for parameter validation
                    crate::analyzer::statements::analyze_statement(ctx, &surrealdb::sql::Statement::Set(set_stmt.clone()))?;
                }
            }
            Entry::Ifelse(if_stmt) => {
                // IF/ELSE branches - analyze all expressions for return types
                for (_condition, body) in &if_stmt.exprs {
                    // Analyze the body value - it could be a block or a single value
                    match body {
                        surrealdb::sql::Value::Block(block) => {
                            let expr_type = analyze_block(ctx, block)?;
                            return_types.push(expr_type);
                        }
                        _ => {
                            // Single value body - treat as implicit return
                            let expr_type = ctx.resolve(body)?;
                            return_types.push(expr_type);
                        }
                    }
                }
                
                // Analyze the close block if present
                if let Some(close_value) = &if_stmt.close {
                    match close_value {
                        surrealdb::sql::Value::Block(block) => {
                            let close_type = analyze_block(ctx, block)?;
                            return_types.push(close_type);
                        }
                        _ => {
                            // Single value else - treat as implicit return
                            let close_type = ctx.resolve(close_value)?;
                            return_types.push(close_type);
                        }
                    }
                }
            }
            _ => {
                // For all other entry types, analyze them as statements
                // This includes SELECT, CREATE, UPDATE, DELETE, etc.
                // We don't care about their return types, but we need to validate parameters
                if let Ok(statement) = entry_to_statement(entry) {
                    // For now, continue with errors instead of failing the entire function analysis
                    // Some statements (like UPDATE $param) may not be fully supported yet
                    let _ = crate::analyzer::statements::analyze_statement(ctx, &statement);
                }
            }
        }
    }

    // If no explicit return statements, functions return null
    if !has_explicit_return {
        return Ok(Kind::Null);
    }

    // If multiple return types, create a union
    if return_types.is_empty() {
        Ok(Kind::Null)
    } else if return_types.len() == 1 {
        Ok(return_types.into_iter().next().unwrap())
    } else {
        // Remove duplicates and create union type
        return_types.sort_by_key(|k| format!("{:?}", k));
        return_types.dedup();
        Ok(Kind::Either(return_types))
    }
}

/// Extracts variable name from LET statements
fn extract_let_variable(set_stmt: &surrealdb::sql::statements::SetStatement) -> Option<String> {
    // LET statements store the variable name as a string
    // Convert to parameter name format (without $)
    let var_name = set_stmt.name.to_string();
    if var_name.starts_with('$') {
        Some(var_name[1..].to_string())
    } else {
        Some(var_name)
    }
}

/// Extracts cast operation: <type> $variable;
fn extract_cast_operation(set_stmt: &surrealdb::sql::statements::SetStatement) -> Option<(String, Kind)> {
    // Cast operations have the pattern: <type> $variable;
    // In SurrealDB, this might be represented as a special SET statement
    // where the name is the variable and the value is the cast type
    
    // Check if this is a cast by examining the structure
    // For now, we'll look for patterns that indicate casting
    match &set_stmt.what {
        surrealdb::sql::Value::Cast(cast) => {
            // This is a cast operation: <type> $variable
            let var_name = set_stmt.name.to_string();
            if var_name.starts_with('$') {
                let param_name = var_name[1..].to_string();
                Some((param_name, cast.0.clone()))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Checks if a declared type is compatible with an inferred type
fn is_compatible_type(declared: &Kind, inferred: &Kind) -> bool {
    match (declared, inferred) {
        // Any is compatible with everything
        (Kind::Any, _) | (_, Kind::Any) => true,
        
        // Exact match
        (a, b) if a == b => true,
        
        // Union types - check if declared is a subset of inferred
        (declared_type, Kind::Either(inferred_variants)) => {
            inferred_variants.iter().any(|variant| is_compatible_type(declared_type, variant))
        }
        
        // Arrays with compatible inner types
        (Kind::Array(declared_inner, _), Kind::Array(inferred_inner, _)) => {
            is_compatible_type(declared_inner, inferred_inner)
        }
        
        // Option types
        (Kind::Option(declared_inner), Kind::Option(inferred_inner)) => {
            is_compatible_type(declared_inner, inferred_inner)
        }
        
        // Declared is more specific than inferred (e.g., string vs any)
        _ => false,
    }
}

/// Converts an Entry to a Statement for analysis
fn entry_to_statement(entry: &Entry) -> Result<surrealdb::sql::Statement, ()> {
    match entry {
        Entry::Select(stmt) => Ok(surrealdb::sql::Statement::Select(stmt.clone())),
        Entry::Create(stmt) => Ok(surrealdb::sql::Statement::Create(stmt.clone())),
        Entry::Update(stmt) => Ok(surrealdb::sql::Statement::Update(stmt.clone())),
        Entry::Delete(stmt) => Ok(surrealdb::sql::Statement::Delete(stmt.clone())),
        Entry::Value(value) => Ok(surrealdb::sql::Statement::Value(value.clone())),
        _ => Err(()),
    }
}