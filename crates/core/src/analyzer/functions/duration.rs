use crate::analyzer::error::{AnalyzerError, AnalyzerResult};
use surrealdb::sql::{Function, Kind};
use super::AnalyzerContext;

pub(super) fn analyze_duration(ctx: &AnalyzerContext, func: &Function) -> AnalyzerResult<Kind> {
    // Get the full function name, e.g. "duration::days" or "duration::from::hours"
    let name = func.name().ok_or(AnalyzerError::UnexpectedSyntax)?;
    let segments: Vec<&str> = name.split("::").collect();

    // We need at least two segments (e.g. "duration" and "days").
    if segments.len() < 2 {
        return Err(AnalyzerError::UnexpectedSyntax);
    }

    // Decide based on the second segment.
    match segments[1] {
        ///////////
        // Counting functions: They expect a duration as input and return a number.
        "days" | "hours" | "micros" | "millis" | "mins" | "nanos" | "secs" | "weeks" | "years" => {
            // Expect one argument: a duration value.
            if let Some(arg) = func.args().first() {
                let arg_kind = ctx.resolve(arg)?;
                // (Assuming your type system provides a duration kind.)
                match arg_kind {
                    Kind::Duration => Ok(Kind::Number),
                    _ => Err(AnalyzerError::UnexpectedSyntax)
                }
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },

        ///////////
        // Conversion functions: They expect a numeric value and return a duration.
        "from" => {
            // We need a third segment to indicate which conversion (e.g. days, hours, etc.)
            let conversion = segments.get(2).ok_or(AnalyzerError::UnexpectedSyntax)?;
            match *conversion {
                "days" | "hours" | "micros" | "millis" | "mins" | "nanos" | "secs" | "weeks" => {
                    if let Some(arg) = func.args().first() {
                        let arg_kind = ctx.resolve(arg)?;
                        // Here we expect a number (this might be either a floating-point or integer type
                        // depending on your implementation).
                        match arg_kind {
                            Kind::Number => Ok(Kind::Duration),
                            _ => Err(AnalyzerError::UnexpectedSyntax)
                        }
                    } else {
                        Err(AnalyzerError::UnexpectedSyntax)
                    }
                },
                _ => Err(AnalyzerError::FunctionNotFound(name.to_string())),
            }
        },

        _ => Err(AnalyzerError::FunctionNotFound(name.to_string())),
    }
}
