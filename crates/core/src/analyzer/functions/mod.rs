use surrealdb::sql::{Function, Kind};
use super::context::AnalyzerContext;
use super::error::{AnalyzerError, AnalyzerResult};

mod crypto;
mod array;
mod math;
mod duration;
mod object;
mod parse;
mod rand;
mod search;
mod types;
mod vector;
mod string;
mod time;
pub mod custom;

pub use custom::analyze_function_definition;

pub fn analyze_function(ctx: &mut AnalyzerContext, func: &Function) -> AnalyzerResult<Kind> {
    let name = func.name().ok_or(AnalyzerError::UnexpectedSyntax)?;

    match name.split("::").next() {
        Some("array") => array::analyze_array(ctx, func),
        Some("crypto") => crypto::analyze_crypto(ctx, func),
        Some("duration") => duration::analyze_duration(ctx, func),
        Some("math") =>  math::analyze_math(ctx, func),
        Some("object") => object::analyze_object(ctx, func),
        Some("parse") => parse::analyze_parse(ctx, func),
        Some("rand") => rand::analyze_rand(ctx, func),
        Some("search") => search::analyze_search(ctx, func),
        Some("type") => types::analyze_type(ctx, func),
        Some("vector") => vector::analyze_vector(ctx, func),
        Some("string") => string::analyze_string(ctx, func),
        Some("time") => time::analyze_time(ctx, func),

        Some("record") => match name.split("::").nth(1) {
            Some("exists") => {
                // record::exists(record_id) -> bool
                if func.args().len() != 1 {
                    return Err(AnalyzerError::UnexpectedSyntax);
                }
                // The argument should be a record ID, but we'll accept any type for now
                let _arg_type = ctx.resolve(func.args().first().unwrap())?;
                Ok(Kind::Bool)
            },
            Some(other) => Err(AnalyzerError::FunctionNotFound(format!("record::{}", other))),
            None => Err(AnalyzerError::UnexpectedSyntax),
        },

        Some("session") => Ok(Kind::String),
        Some("sleep") => Ok(Kind::Null),
        Some("count") => Ok(Kind::Int),

        Some("meta") => match name.split("::").nth(1) {
            Some("id") | Some("tb") => Ok(Kind::String),
            _ => Err(AnalyzerError::FunctionNotFound(name.to_string())),
        },

        Some("encoding") => match (name.split("::").nth(1), name.split("::").nth(2)) {
            (Some("base64"), Some("encode")) => Ok(Kind::String),
            (Some("base64"), Some("decode")) => Ok(Kind::Bytes),
            _ => Err(AnalyzerError::FunctionNotFound(name.to_string())),
        },

        Some("http") => match name.split("::").nth(1) {
            Some("head") => Ok(Kind::Null),
            Some(method) if ["get", "put", "post", "patch", "delete"].contains(&method) => {
                Ok(Kind::Object)
            }
            _ => Err(AnalyzerError::FunctionNotFound(name.to_string())),
        },

        Some("fn") => {
            // Custom function - look it up in the context
            if let Some(func_def) = ctx.find_function_definition(name) {
                // Clone the function definition to avoid borrowing issues
                let func_def = func_def.clone();
                
                // Validate function call parameters
                validate_function_call(ctx, func, &func_def)?;
                
                // Inherit required parameters from this function
                ctx.inherit_function_required_params(name);
                
                // Return the analyzed type (should have been analyzed during schema phase)
                if let Some(returns) = &func_def.returns {
                    Ok(returns.clone())
                } else {
                    // If no return type was inferred/specified, return Any for now
                    // TODO: This should have been analyzed during schema phase
                    Ok(Kind::Any)
                }
            } else {
                // Function not found - show what we were looking for
                Err(AnalyzerError::FunctionNotFound(format!("Custom function '{}' not found", name)))
            }
        }

        Some(_) | None => {
            // Check if this is a custom function without fn:: prefix
            let full_name = format!("fn::{}", name);
            if let Some(func_def) = ctx.find_function_definition(&full_name) {
                // Clone the function definition to avoid borrowing issues
                let func_def = func_def.clone();
                
                // Validate function call parameters
                validate_function_call(ctx, func, &func_def)?;
                
                // Inherit required parameters from this function
                ctx.inherit_function_required_params(&full_name);
                
                // Return the analyzed type (should have been analyzed during schema phase)
                if let Some(returns) = &func_def.returns {
                    Ok(returns.clone())
                } else {
                    // If no return type was inferred/specified, return Any for now
                    // TODO: This should have been analyzed during schema phase
                    Ok(Kind::Any)
                }
            } else {
                Err(AnalyzerError::FunctionNotFound(name.to_string()))
            }
        },
    }
}

/// Validates function call parameters against the function definition
fn validate_function_call(ctx: &mut AnalyzerContext, func: &Function, func_def: &surrealdb::sql::statements::DefineFunctionStatement) -> AnalyzerResult<()> {
    let args = func.args();
    let expected_params = &func_def.args;
    
    // Check argument count
    if args.len() != expected_params.len() {
        return Err(AnalyzerError::InvalidFunctionCall {
            function: func_def.name.0.clone(),
            message: format!("Expected {} arguments, got {}", expected_params.len(), args.len()),
        });
    }
    
    // Check argument types
    for (i, (arg_value, (param_name, expected_type))) in args.iter().zip(expected_params.iter()).enumerate() {
        let arg_type = ctx.resolve(arg_value)?;
        
        // For now, do basic type compatibility checking
        if !is_argument_compatible(&arg_type, expected_type) {
            return Err(AnalyzerError::InvalidFunctionCall {
                function: func_def.name.0.clone(),
                message: format!("Argument {} ({}): expected {:?}, got {:?}", i + 1, param_name.0, expected_type, arg_type),
            });
        }
    }
    
    Ok(())
}

/// Checks if an argument type is compatible with a parameter type
fn is_argument_compatible(arg_type: &Kind, param_type: &Kind) -> bool {
    match (arg_type, param_type) {
        // Any is compatible with everything
        (Kind::Any, _) | (_, Kind::Any) => true,
        
        // Exact match
        (a, b) if a == b => true,
        
        // String literals are compatible with string parameters
        (Kind::Literal(surrealdb::sql::Literal::String(_)), Kind::String) => true,
        
        // Number literals are compatible with number parameters
        (Kind::Literal(surrealdb::sql::Literal::Number(_)), Kind::Number) => true,
        
        // Arrays with compatible inner types
        (Kind::Array(arg_inner, _), Kind::Array(param_inner, _)) => {
            is_argument_compatible(arg_inner, param_inner)
        }
        
        // Option types
        (Kind::Option(arg_inner), Kind::Option(param_inner)) => {
            is_argument_compatible(arg_inner, param_inner)
        }
        
        // Non-option can be passed to option parameter
        (arg_type, Kind::Option(param_inner)) => {
            is_argument_compatible(arg_type, param_inner)
        }
        
        // Records are compatible if they reference the same table
        (Kind::Record(arg_tables), Kind::Record(param_tables)) => {
            arg_tables == param_tables
        }
        
        // Otherwise, not compatible
        _ => false,
    }
}




