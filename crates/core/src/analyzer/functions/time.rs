use crate::analyzer::error::{AnalyzerError, AnalyzerResult};
use surrealdb::sql::{Function, Kind};
use super::AnalyzerContext;

/// Analyze functions in the "time" namespace.
///
/// This covers functions for working with datetime values. Many time functions accept an optional datetime
/// (defaulting to “now” if none is given), and some perform conversions from numeric values.
///
/// Supported functions include (but are not limited to):
///   • time::ceil(datetime, duration) -> datetime
///   • time::day(option<datetime>) -> number
///   • time::floor(datetime, duration) -> datetime
///   • time::format(datetime, string) -> string
///   • time::group(datetime, string) -> datetime
///   • time::hour(option<datetime>) -> number
///   • time::max(array<datetime>) -> datetime
///   • time::micros(option<datetime>) -> number
///   • time::millis(option<datetime>) -> number
///   • time::min(array<datetime>) -> datetime
///   • time::minute(option<datetime>) -> number
///   • time::month(option<datetime>) -> number
///   • time::nano(option<datetime>) -> number
///   • time::now() -> datetime
///   • time::round(datetime, duration) -> datetime
///   • time::second(option<datetime>) -> number
///   • time::timezone() -> string
///   • time::unix(option<datetime>) -> number
///   • time::wday(option<datetime>) -> number
///   • time::week(option<datetime>) -> number
///   • time::yday(option<datetime>) -> number
///   • time::year(option<datetime>) -> number
///   • time::is::leap_year(datetime) -> bool
///
/// Conversion functions:
///   • time::from::micros(number) -> datetime
///   • time::from::millis(number) -> datetime
///   • time::from::nanos(number) -> datetime
///   • time::from::secs(number) -> datetime
///   • time::from::unix(number) -> datetime
///   • time::from::ulid(string) -> datetime
///   • time::from::uuid(uuid) -> datetime
///
pub(super) fn analyze_time(ctx: &AnalyzerContext, func: &Function) -> AnalyzerResult<Kind> {
    // Retrieve the full function name (e.g. "time::ceil", "time::from::millis", etc.)
    let name = func.name().ok_or(AnalyzerError::UnexpectedSyntax)?;
    let segments: Vec<&str> = name.split("::").collect();

    // Make sure the function is in the time namespace.
    if segments.is_empty() || segments[0] != "time" {
        return Err(AnalyzerError::FunctionNotFound(name.to_string()));
    }

    match segments.get(1) {
        // time::ceil(datetime, duration) -> datetime
        Some(&"ceil") => {
            let args = func.args();
            if args.len() != 2 { return Err(AnalyzerError::UnexpectedSyntax); }
            match (ctx.resolve(&args[0])?, ctx.resolve(&args[1])?) {
                (Kind::Datetime, Kind::Duration) => Ok(Kind::Datetime),
                _ => Err(AnalyzerError::UnexpectedSyntax),
            }
        },

        // time::day(option<datetime>) -> number
        Some(&"day") => {
            let args = func.args();
            if args.len() > 1 { return Err(AnalyzerError::UnexpectedSyntax); }
            if args.len() == 1 {
                if ctx.resolve(args.first().unwrap())? != Kind::Datetime {
                    return Err(AnalyzerError::UnexpectedSyntax);
                }
            }
            Ok(Kind::Number)
        },

        // time::floor(datetime, duration) -> datetime
        Some(&"floor") => {
            let args = func.args();
            if args.len() != 2 { return Err(AnalyzerError::UnexpectedSyntax); }
            match (ctx.resolve(&args[0])?, ctx.resolve(&args[1])?) {
                (Kind::Datetime, Kind::Duration) => Ok(Kind::Datetime),
                _ => Err(AnalyzerError::UnexpectedSyntax),
            }
        },

        // time::format(datetime, string) -> string
        Some(&"format") => {
            let args = func.args();
            if args.len() != 2 { return Err(AnalyzerError::UnexpectedSyntax); }
            if ctx.resolve(&args[0])? != Kind::Datetime || ctx.resolve(&args[1])? != Kind::String {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            Ok(Kind::String)
        },

        // time::group(datetime, string) -> datetime
        Some(&"group") => {
            let args = func.args();
            if args.len() != 2 { return Err(AnalyzerError::UnexpectedSyntax); }
            if ctx.resolve(&args[0])? != Kind::Datetime || ctx.resolve(&args[1])? != Kind::String {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            Ok(Kind::Datetime)
        },

        // time::hour(option<datetime>) -> number
        Some(&"hour") => {
            let args = func.args();
            if args.len() > 1 { return Err(AnalyzerError::UnexpectedSyntax); }
            if args.len() == 1 && ctx.resolve(args.first().unwrap())? != Kind::Datetime {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            Ok(Kind::Number)
        },

        // time::max(array<datetime>) -> datetime
        Some(&"max") => {
            let args = func.args();
            if args.len() != 1 { return Err(AnalyzerError::UnexpectedSyntax); }
            match ctx.resolve(args.first().unwrap())? {
                Kind::Array(inner, _) => {
                    // For our purposes we check the inner type is a datetime.
                    if *inner != Kind::Datetime {
                        return Err(AnalyzerError::UnexpectedSyntax);
                    }
                    Ok(Kind::Datetime)
                },
                _ => Err(AnalyzerError::UnexpectedSyntax),
            }
        },

        // time::micros(option<datetime>) -> number
        Some(&"micros") => {
            let args = func.args();
            if args.len() > 1 { return Err(AnalyzerError::UnexpectedSyntax); }
            if args.len() == 1 && ctx.resolve(args.first().unwrap())? != Kind::Datetime {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            Ok(Kind::Number)
        },

        // time::millis(option<datetime>) -> number
        Some(&"millis") => {
            let args = func.args();
            if args.len() > 1 { return Err(AnalyzerError::UnexpectedSyntax); }
            if args.len() == 1 && ctx.resolve(args.first().unwrap())? != Kind::Datetime {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            Ok(Kind::Number)
        },

        // time::min(array<datetime>) -> datetime
        Some(&"min") => {
            let args = func.args();
            if args.len() != 1 { return Err(AnalyzerError::UnexpectedSyntax); }
            match ctx.resolve(args.first().unwrap())? {
                Kind::Array(inner, _) => {
                    if *inner != Kind::Datetime {
                        return Err(AnalyzerError::UnexpectedSyntax);
                    }
                    Ok(Kind::Datetime)
                },
                _ => Err(AnalyzerError::UnexpectedSyntax),
            }
        },

        // time::minute(option<datetime>) -> number
        Some(&"minute") => {
            let args = func.args();
            if args.len() > 1 { return Err(AnalyzerError::UnexpectedSyntax); }
            if args.len() == 1 && ctx.resolve(args.first().unwrap())? != Kind::Datetime {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            Ok(Kind::Number)
        },

        // time::month(option<datetime>) -> number
        Some(&"month") => {
            let args = func.args();
            if args.len() > 1 { return Err(AnalyzerError::UnexpectedSyntax); }
            if args.len() == 1 && ctx.resolve(args.first().unwrap())? != Kind::Datetime {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            Ok(Kind::Number)
        },

        // time::nano(option<datetime>) -> number
        Some(&"nano") => {
            let args = func.args();
            if args.len() > 1 { return Err(AnalyzerError::UnexpectedSyntax); }
            if args.len() == 1 && ctx.resolve(args.first().unwrap())? != Kind::Datetime {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            Ok(Kind::Number)
        },

        // time::now() -> datetime
        Some(&"now") => {
            if !func.args().is_empty() {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            Ok(Kind::Datetime)
        },

        // time::round(datetime, duration) -> datetime
        Some(&"round") => {
            let args = func.args();
            if args.len() != 2 { return Err(AnalyzerError::UnexpectedSyntax); }
            match (ctx.resolve(&args[0])?, ctx.resolve(&args[1])?) {
                (Kind::Datetime, Kind::Duration) => Ok(Kind::Datetime),
                _ => Err(AnalyzerError::UnexpectedSyntax),
            }
        },

        // time::second(option<datetime>) -> number
        Some(&"second") => {
            let args = func.args();
            if args.len() > 1 { return Err(AnalyzerError::UnexpectedSyntax); }
            if args.len() == 1 && ctx.resolve(args.first().unwrap())? != Kind::Datetime {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            Ok(Kind::Number)
        },

        // time::timezone() -> string
        Some(&"timezone") => {
            if !func.args().is_empty() {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            Ok(Kind::String)
        },

        // time::unix(option<datetime>) -> number
        Some(&"unix") => {
            let args = func.args();
            if args.len() > 1 { return Err(AnalyzerError::UnexpectedSyntax); }
            if args.len() == 1 && ctx.resolve(args.first().unwrap())? != Kind::Datetime {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            Ok(Kind::Number)
        },

        // time::wday(option<datetime>) -> number
        Some(&"wday") => {
            let args = func.args();
            if args.len() > 1 { return Err(AnalyzerError::UnexpectedSyntax); }
            if args.len() == 1 && ctx.resolve(args.first().unwrap())? != Kind::Datetime {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            Ok(Kind::Number)
        },

        // time::week(option<datetime>) -> number
        Some(&"week") => {
            let args = func.args();
            if args.len() > 1 { return Err(AnalyzerError::UnexpectedSyntax); }
            if args.len() == 1 && ctx.resolve(args.first().unwrap())? != Kind::Datetime {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            Ok(Kind::Number)
        },

        // time::yday(option<datetime>) -> number
        Some(&"yday") => {
            let args = func.args();
            if args.len() > 1 { return Err(AnalyzerError::UnexpectedSyntax); }
            if args.len() == 1 && ctx.resolve(args.first().unwrap())? != Kind::Datetime {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            Ok(Kind::Number)
        },

        // time::year(option<datetime>) -> number
        Some(&"year") => {
            let args = func.args();
            if args.len() > 1 { return Err(AnalyzerError::UnexpectedSyntax); }
            if args.len() == 1 && ctx.resolve(args.first().unwrap())? != Kind::Datetime {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            Ok(Kind::Number)
        },

        // For testing leap years: time::is::leap_year(datetime) -> bool
        Some(&"is") => {
            if segments.len() < 3 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            match segments[2] {
                "leap_year" => {
                    let args = func.args();
                    if args.len() != 1 { return Err(AnalyzerError::UnexpectedSyntax); }
                    if ctx.resolve(args.first().unwrap())? != Kind::Datetime {
                        return Err(AnalyzerError::UnexpectedSyntax);
                    }
                    Ok(Kind::Bool)
                },
                other => Err(AnalyzerError::FunctionNotFound(format!("time::is::{}", other))),
            }
        },

        // Time conversion functions under the "from" sub-namespace.
        Some(&"from") => {
            if segments.len() < 3 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            match segments[2] {
                // These expect one numeric argument and return a datetime.
                "micros" | "millis" | "nanos" | "secs" | "unix" => {
                    let args = func.args();
                    if args.len() != 1 { return Err(AnalyzerError::UnexpectedSyntax); }
                    if ctx.resolve(args.first().unwrap())? != Kind::Number {
                        return Err(AnalyzerError::UnexpectedSyntax);
                    }
                    Ok(Kind::Datetime)
                },
                // For ULID and UUID conversions.
                "ulid" => {
                    let args = func.args();
                    if args.len() != 1 { return Err(AnalyzerError::UnexpectedSyntax); }
                    // Here we expect a string representing a ULID.
                    if ctx.resolve(args.first().unwrap())? != Kind::String {
                        return Err(AnalyzerError::UnexpectedSyntax);
                    }
                    Ok(Kind::Datetime)
                },
                "uuid" => {
                    let args = func.args();
                    if args.len() != 1 { return Err(AnalyzerError::UnexpectedSyntax); }
                    // Here we expect a UUID type.
                    if ctx.resolve(args.first().unwrap())? != Kind::Uuid {
                        return Err(AnalyzerError::UnexpectedSyntax);
                    }
                    Ok(Kind::Datetime)
                },
                other => Err(AnalyzerError::FunctionNotFound(format!("time::from::{}", other))),
            }
        },

        // If no matching subcommand is found, return an error.
        Some(other) => Err(AnalyzerError::FunctionNotFound(format!("time::{}", other))),
        None => Err(AnalyzerError::UnexpectedSyntax),
    }
}
