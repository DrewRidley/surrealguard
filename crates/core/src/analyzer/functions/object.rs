use crate::analyzer::error::{AnalyzerError, AnalyzerResult};
use surrealdb::sql::{Function, Kind};
use super::AnalyzerContext;

/// Analyze functions operating on objects.
///
/// Supported functions:
/// • object::entries(object) -> array of [string, any]
/// • object::from_entries(array) -> object
/// • object::is_empty(object) -> bool
/// • object::keys(object) -> array<string>
/// • object::len(object) -> number
/// • object::values(object) -> array<any>
pub(super) fn analyze_object(ctx: &mut AnalyzerContext, func: &Function) -> AnalyzerResult<Kind> {
    // Get the full function name, e.g. "object::entries"
    let name = func.name().ok_or(AnalyzerError::UnexpectedSyntax)?;
    // Get the specific function name after the "object" namespace.
    let sub = name.split("::").nth(1).ok_or(AnalyzerError::UnexpectedSyntax)?;

    // Helper: check that we have exactly one argument and return a reference to it.
    let arg = if func.args().len() == 1 {
        func.args().first().unwrap()
    } else {
        return Err(AnalyzerError::UnexpectedSyntax);
    };

    match sub {
        "entries" => {
            // object::entries(object) -> array of entry pairs
            // Ensure the provided argument is an object.
            match ctx.resolve(arg)? {
                Kind::Object => {
                    // Return an array where each entry is itself an array of two elements:
                    // a string (the key) and an unknown/dynamic type (the value).
                    // (We assume Kind::Any exists to represent an unconstrained type.)
                    let entry_type = Kind::Array(Box::new(Kind::Any), None);
                    Ok(Kind::Array(Box::new(entry_type), None))
                },
                _ => Err(AnalyzerError::UnexpectedSyntax),
            }
        },

        "from_entries" => {
            // object::from_entries(array) -> object
            // Expect an array (with inner type array-of-[string, any] pairs) as input.
            match ctx.resolve(arg)? {
                Kind::Array(_, _) => Ok(Kind::Object),
                _ => Err(AnalyzerError::UnexpectedSyntax),
            }
        },

        "is_empty" => {
            // object::is_empty(object) -> bool
            match ctx.resolve(arg)? {
                Kind::Object => Ok(Kind::Bool),
                _ => Err(AnalyzerError::UnexpectedSyntax),
            }
        },

        "keys" => {
            // object::keys(object) -> array<string>
            match ctx.resolve(arg)? {
                Kind::Object => Ok(Kind::Array(Box::new(Kind::String), None)),
                _ => Err(AnalyzerError::UnexpectedSyntax),
            }
        },

        "len" => {
            // object::len(object) -> number
            match ctx.resolve(arg)? {
                Kind::Object => Ok(Kind::Number),
                _ => Err(AnalyzerError::UnexpectedSyntax),
            }
        },

        "values" => {
            // object::values(object) -> array<any>
            match ctx.resolve(arg)? {
                Kind::Object => Ok(Kind::Array(Box::new(Kind::Any), None)),
                _ => Err(AnalyzerError::UnexpectedSyntax),
            }
        },

        other => Err(AnalyzerError::FunctionNotFound(format!("object::{}", other))),
    }
}
