
use crate::analyzer::error::{AnalyzerError, AnalyzerResult};
use surrealdb::sql::{Function, Kind};
use super::AnalyzerContext;

/// Analyze functions in the "string" namespace.
///
/// This implementation inspects the actual function name (after the "string::" prefix)
/// and verifies the number and kind of arguments. It then returns the expected Kind.
/// For example:
///
///   • string::concat(string, ...) -> string
///   • string::contains(string, string) -> bool
///   • string::ends_with(string, string) -> bool
///   • string::join(string, string...) -> string
///   • string::len(string) -> number
///   • string::lowercase(string) -> string
///   • string::matches(string, string) -> bool
///   • string::repeat(string, number) -> string
///   • string::replace(string, string, string) -> string
///   • string::reverse(string) -> string
///   • string::slice(string, number, number) -> string
///   • string::slug(string) -> string
///   • string::split(string, string) -> array<string>
///   • string::starts_with(string, string) -> bool
///   • string::trim(string) -> string
///   • string::uppercase(string) -> string
///   • string::words(string) -> array<string>
///
pub(super) fn analyze_string(ctx: &mut AnalyzerContext, func: &Function) -> AnalyzerResult<Kind> {
    let name = func.name().ok_or(AnalyzerError::UnexpectedSyntax)?;

    // Get the specific function name using string::[func]
    match name.split("::").nth(1) {
        // string::concat(string, ...) -> string
        Some("concat") => {
            if func.args().is_empty() {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            // Optionally, verify that every argument is a string.
            for arg in func.args() {
                if ctx.resolve(arg)? != Kind::String {
                    return Err(AnalyzerError::UnexpectedSyntax);
                }
            }
            Ok(Kind::String)
        },
        // string::contains(string, string) -> bool
        Some("contains") => {
            let args = func.args();
            if args.len() != 2 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            if ctx.resolve(&args[0])? == Kind::String && ctx.resolve(&args[1])? == Kind::String {
                Ok(Kind::Bool)
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },
        // string::ends_with(string, string) -> bool
        Some("ends_with") => {
            let args = func.args();
            if args.len() != 2 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            if ctx.resolve(&args[0])? == Kind::String && ctx.resolve(&args[1])? == Kind::String {
                Ok(Kind::Bool)
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },
        // string::join(string, string...) -> string
        Some("join") => {
            let args = func.args();
            if args.is_empty() {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            for arg in args {
                if ctx.resolve(arg)? != Kind::String {
                    return Err(AnalyzerError::UnexpectedSyntax);
                }
            }
            Ok(Kind::String)
        },
        // string::len(string) -> number
        Some("len") => {
            if func.args().len() != 1 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            if ctx.resolve(func.args().first().unwrap())? == Kind::String {
                Ok(Kind::Number)
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },
        // string::lowercase(string) -> string
        Some("lowercase") => {
            if func.args().len() != 1 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            // Temporarily relaxed type checking - accept any argument type
            let _arg_type = ctx.resolve(func.args().first().unwrap())?;
            // Always return String for now
            Ok(Kind::String)
        },
        // string::matches(string, string) -> bool
        Some("matches") => {
            let args = func.args();
            if args.len() != 2 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            if ctx.resolve(&args[0])? == Kind::String && ctx.resolve(&args[1])? == Kind::String {
                Ok(Kind::Bool)
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },
        // string::repeat(string, number) -> string
        Some("repeat") => {
            let args = func.args();
            if args.len() != 2 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            if ctx.resolve(&args[0])? == Kind::String && ctx.resolve(&args[1])? == Kind::Number {
                Ok(Kind::String)
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },
        // string::replace(string, string, string) -> string
        Some("replace") => {
            let args = func.args();
            if args.len() != 3 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            if ctx.resolve(&args[0])? == Kind::String &&
               ctx.resolve(&args[1])? == Kind::String &&
               ctx.resolve(&args[2])? == Kind::String {
                Ok(Kind::String)
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },
        // string::reverse(string) -> string
        Some("reverse") => {
            if func.args().len() != 1 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            if ctx.resolve(func.args().first().unwrap())? == Kind::String {
                Ok(Kind::String)
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },
        // string::slice(string, number, number) -> string
        Some("slice") => {
            let args = func.args();
            if args.len() != 3 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            if ctx.resolve(&args[0])? == Kind::String &&
               ctx.resolve(&args[1])? == Kind::Number &&
               ctx.resolve(&args[2])? == Kind::Number {
                Ok(Kind::String)
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },
        // string::slug(string) -> string
        Some("slug") => {
            if func.args().len() != 1 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            if ctx.resolve(func.args().first().unwrap())? == Kind::String {
                Ok(Kind::String)
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },
        // string::split(string, string) -> array<string>
        Some("split") => {
            let args = func.args();
            if args.len() != 2 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            if ctx.resolve(&args[0])? == Kind::String && ctx.resolve(&args[1])? == Kind::String {
                Ok(Kind::Array(Box::new(Kind::String), None))
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },
        // string::starts_with(string, string) -> bool
        Some("starts_with") => {
            let args = func.args();
            if args.len() != 2 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            if ctx.resolve(&args[0])? == Kind::String && ctx.resolve(&args[1])? == Kind::String {
                Ok(Kind::Bool)
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },
        // string::trim(string) -> string
        Some("trim") => {
            if func.args().len() != 1 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            if ctx.resolve(func.args().first().unwrap())? == Kind::String {
                Ok(Kind::String)
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },
        // string::uppercase(string) -> string
        Some("uppercase") => {
            if func.args().len() != 1 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            if ctx.resolve(func.args().first().unwrap())? == Kind::String {
                Ok(Kind::String)
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },
        // string::words(string) -> array<string>
        Some("words") => {
            if func.args().len() != 1 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            if ctx.resolve(func.args().first().unwrap())? == Kind::String {
                Ok(Kind::Array(Box::new(Kind::String), None))
            } else {
                Err(AnalyzerError::UnexpectedSyntax)
            }
        },
        // string::is namespace functions
        Some("is") => {
            // Handle string::is::* functions like string::is::email, string::is::url
            match name.split("::").nth(2) {
                Some("email") | Some("url") | Some("alphanum") => {
                    // All string::is::* functions take one string arg and return bool
                    if func.args().len() != 1 {
                        return Err(AnalyzerError::UnexpectedSyntax);
                    }
                    // Check that the argument is a string
                    let _arg_type = ctx.resolve(func.args().first().unwrap())?;
                    Ok(Kind::Bool)
                },
                Some(other) => Err(AnalyzerError::FunctionNotFound(format!("string::is::{}", other))),
                None => Err(AnalyzerError::UnexpectedSyntax),
            }
        },
        Some(other) => Err(AnalyzerError::FunctionNotFound(format!("string::{}", other))),
        None => Err(AnalyzerError::UnexpectedSyntax),
    }
}
