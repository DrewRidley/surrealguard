
use crate::analyzer::error::{AnalyzerError, AnalyzerResult};
use surrealdb::sql::{Function, Kind};
use super::AnalyzerContext;

/// Analyze functions operating in the "parse" namespace.
/// Supported functions:
///   • parse::email::host(string) -> string
///   • parse::email::user(string) -> string
///   • parse::url::domain(string) -> string
///   • parse::url::fragment(string) -> string
///   • parse::url::host(string) -> string
///   • parse::url::path(string) -> string
///   • parse::url::port(string) -> number
///   • parse::url::scheme(string) -> string
///   • parse::url::query(string) -> string
pub(super) fn analyze_parse(ctx: &AnalyzerContext, func: &Function) -> AnalyzerResult<Kind> {
    // Retrieve the full function name, e.g. "parse::email::host" or "parse::url::port"
    let name = func.name().ok_or(AnalyzerError::UnexpectedSyntax)?;
    let segments: Vec<&str> = name.split("::").collect();

    // For parse functions, we expect at least three segments: "parse", the module (e.g. "email" or "url"),
    // and then the specific function (e.g. "host", "user", etc.)
    if segments.len() < 3 {
        return Err(AnalyzerError::UnexpectedSyntax);
    }

    // Ensure that exactly one argument is provided.
    let arg = if func.args().len() == 1 {
        func.args().first().unwrap()
    } else {
        return Err(AnalyzerError::UnexpectedSyntax);
    };

    // All parse functions expect a string argument.
    match ctx.resolve(arg)? {
        Kind::String => (),
        _ => return Err(AnalyzerError::UnexpectedSyntax),
    };

    // Identify the submodule and subcommand.
    match segments[1] {
        "email" => {
            // Supported: parse::email::host, parse::email::user
            match segments[2] {
                "host" | "user" => Ok(Kind::String),
                other => Err(AnalyzerError::FunctionNotFound(format!("parse::email::{}", other))),
            }
        },
        "url" => {
            // Supported: parse::url::domain, parse::url::fragment, parse::url::host, parse::url::path,
            //            parse::url::port, parse::url::scheme, parse::url::query.
            match segments[2] {
                "domain" | "fragment" | "host" | "path" | "scheme" | "query" => Ok(Kind::String),
                "port" => Ok(Kind::Number),
                other => Err(AnalyzerError::FunctionNotFound(format!("parse::url::{}", other))),
            }
        },
        other => Err(AnalyzerError::FunctionNotFound(format!("parse::{}", other))),
    }
}
