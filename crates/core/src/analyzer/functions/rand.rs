use crate::analyzer::error::{AnalyzerError, AnalyzerResult};
use surrealdb::sql::{Function, Kind};
use super::AnalyzerContext;

pub(super) fn analyze_rand(ctx: &AnalyzerContext, func: &Function) -> AnalyzerResult<Kind> {
    let name = func.name().ok_or(AnalyzerError::UnexpectedSyntax)?;
    let segments: Vec<&str> = name.split("::").collect();

    // If function is just "rand()", return a random float.
    if segments.len() == 1 && segments[0] == "rand" {
        if !func.args().is_empty() {
            return Err(AnalyzerError::UnexpectedSyntax);
        }
        return Ok(Kind::Number);
    }

    if segments.get(0) != Some(&"rand") {
        return Err(AnalyzerError::FunctionNotFound(name.to_string()));
    }

    // Handle functions at the "rand::<sub>" level.
    if segments.len() == 2 {
        let sub = segments[1];
        return match sub {
            "bool" => {
                if !func.args().is_empty() {
                    Err(AnalyzerError::UnexpectedSyntax)
                } else {
                    Ok(Kind::Bool)
                }
            }
            "enum" => {
                // Expects at least one argument; returns Kind::Any.
                if func.args().is_empty() {
                    Err(AnalyzerError::UnexpectedSyntax)
                } else {
                    Ok(Kind::Any)
                }
            }
            "float" => {
                let args = func.args();
                match args.len() {
                    0 => Ok(Kind::Number),
                    2 => {
                        let first = ctx.resolve(&args[0])?;
                        let second = ctx.resolve(&args[1])?;
                        match (first, second) {
                            (Kind::Number, Kind::Number) => Ok(Kind::Number),
                            _ => Err(AnalyzerError::UnexpectedSyntax),
                        }
                    }
                    _ => Err(AnalyzerError::UnexpectedSyntax),
                }
            }
            // ... additional cases like "guid", "int", "string", etc.
            other => Err(AnalyzerError::FunctionNotFound(format!("rand::{}", other))),
        };
    }

    // Handle functions with more segments (e.g. "rand::uuid::v4", "rand::ulid", etc.)
    if segments.len() >= 3 {
        return match segments[1] {
            "uuid" => match segments[2] {
                "v4" | "v7" => {
                    let args = func.args();
                    match args.len() {
                        0 => Ok(Kind::Uuid),
                        1 => match ctx.resolve(&args[0])? {
                            Kind::Datetime => Ok(Kind::Uuid),
                            _ => Err(AnalyzerError::UnexpectedSyntax),
                        },
                        _ => Err(AnalyzerError::UnexpectedSyntax),
                    }
                }
                other => Err(AnalyzerError::FunctionNotFound(format!("rand::uuid::{}", other))),
            },
            "ulid" => {
                let args = func.args();
                match args.len() {
                    0 => Ok(Kind::Uuid), // or Kind::Ulid if available
                    1 => match ctx.resolve(&args[0])? {
                        Kind::Datetime => Ok(Kind::Uuid),
                        _ => Err(AnalyzerError::UnexpectedSyntax),
                    },
                    _ => Err(AnalyzerError::UnexpectedSyntax),
                }
            }
            other => Err(AnalyzerError::FunctionNotFound(format!("rand::{}", other))),
        };
    }

    Err(AnalyzerError::FunctionNotFound(name.to_string()))
}
