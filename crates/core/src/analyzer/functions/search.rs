use crate::analyzer::error::{AnalyzerError, AnalyzerResult};
use surrealdb::sql::{Function, Kind};
use super::AnalyzerContext;

/// Analyze functions operating in the "search" namespace.
///
/// Supported functions:
///   • search::analyze(analyzer, string) -> array<string>
///   • search::score(number) -> number
///   • search::highlight(string, string, number, [boolean]) -> string
///   • search::offsets(number, [boolean]) -> object
pub(super) fn analyze_search(ctx: &mut AnalyzerContext, func: &Function) -> AnalyzerResult<Kind> {
    // Retrieve the full function name, e.g. "search::analyze"
    let name = func.name().ok_or(AnalyzerError::UnexpectedSyntax)?;
    let segments: Vec<&str> = name.split("::").collect();

    // We expect at least two segments: "search" and the command.
    if segments.len() < 2 {
        return Err(AnalyzerError::UnexpectedSyntax);
    }

    match segments[1] {
        "analyze" => {
            // API: search::analyze(analyzer, string) -> array<string>
            let args = func.args();
            if args.len() != 2 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            // Both arguments should be strings.
            match (ctx.resolve(&args[0])?, ctx.resolve(&args[1])?) {
                (Kind::String, Kind::String) => {
                    // Return an array of strings.
                    Ok(Kind::Array(Box::new(Kind::String), None))
                }
                _ => Err(AnalyzerError::UnexpectedSyntax),
            }
        }

        "score" => {
            // API: search::score(number) -> number
            let args = func.args();
            if args.len() != 1 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            if let Kind::Number = ctx.resolve(&args[0])? {
                Ok(Kind::Number)
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        }

        "highlight" => {
            // API: search::highlight(prefix, suffix, predicate_ref, [boolean]) -> string
            let args = func.args();
            // Expect either 3 or 4 arguments.
            if args.len() < 3 || args.len() > 4 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            // First argument: prefix string.
            if let Kind::String = ctx.resolve(&args[0])? {} else {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            // Second argument: suffix string.
            if let Kind::String = ctx.resolve(&args[1])? {} else {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            // Third argument: number (predicate reference).
            if let Kind::Number = ctx.resolve(&args[2])? {} else {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            // Fourth argument (if given): boolean.
            if args.len() == 4 {
                if let Kind::Bool = ctx.resolve(&args[3])? {} else {
                    return Err(AnalyzerError::UnexpectedSyntax);
                }
            }
            // Return a string (even though the runtime might return either a string or an array of strings).
            Ok(Kind::String)
        }

        "offsets" => {
            // API: search::offsets(number, [boolean]) -> object
            let args = func.args();
            // Expect either 1 or 2 arguments.
            if args.len() < 1 || args.len() > 2 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            // First argument must be a number.
            if let Kind::Number = ctx.resolve(&args[0])? {
            } else {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            // Optional second argument must be a boolean.
            if args.len() == 2 {
                if let Kind::Bool = ctx.resolve(&args[1])? {
                } else {
                    return Err(AnalyzerError::UnexpectedSyntax);
                }
            }
            Ok(Kind::Object)
        }

        other => Err(AnalyzerError::FunctionNotFound(format!("search::{}", other))),
    }
}
