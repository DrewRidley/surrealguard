use crate::analyzer::error::{AnalyzerError, AnalyzerResult};
use surrealdb::sql::{Function, Kind};
use super::AnalyzerContext;

pub(super) fn analyze_array(ctx: &AnalyzerContext, func: &Function) -> AnalyzerResult<Kind> {
    let name = func.name().ok_or(AnalyzerError::UnexpectedSyntax)?;

    // Get the specific function after the namespace
    match name.split("::").nth(1) {
        // Return array<bool>
        Some("all" | "any" | "is_empty" | "matches") => Ok(Kind::Bool),

        // Return original array type
        Some("add" | "append" | "combine" | "fill" | "insert" |
             "prepend" | "push" | "remove" | "reverse" | "shuffle" |
             "sort" | "sort::asc" | "sort::desc" | "swap") => {
            if let Some(first_arg) = func.args().first() {
                ctx.resolve(first_arg)
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },

        // Return single element from array
        Some("at" | "find" | "first" | "last" | "pop") => {
            if let Some(first_arg) = func.args().first() {
                match ctx.resolve(first_arg)? {
                    Kind::Array(inner_type, _) => Ok(*inner_type),
                    _ => Err(AnalyzerError::UnexpectedSyntax),
                }
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },

        // Return flattened array
        Some("flatten") => {
            if let Some(first_arg) = func.args().first() {
                match ctx.resolve(first_arg)? {
                    Kind::Array(inner_type, _) => {
                        match *inner_type {
                            Kind::Array(most_inner, _) => Ok(Kind::Array(most_inner, None)),
                            _ => Ok(Kind::Array(inner_type, None))
                        }
                    },
                    _ => Err(AnalyzerError::UnexpectedSyntax),
                }
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },

        // Return array<number>
        Some("range") => Ok(Kind::Array(Box::new(Kind::Number), None)),

        // Return string
        Some("join") => Ok(Kind::String),

        // Return number
        Some("len") => Ok(Kind::Number),

        // Return array (preserving inner type if possible)
        Some("distinct" | "filter" | "group" | "slice" | "windows") => {
            if let Some(first_arg) = func.args().first() {
                ctx.resolve(first_arg)
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },

        // Not a valid array function
        Some(other) => Err(AnalyzerError::FunctionNotFound(format!("array::{}", other))),
        None => Err(AnalyzerError::UnexpectedSyntax),
    }
}
