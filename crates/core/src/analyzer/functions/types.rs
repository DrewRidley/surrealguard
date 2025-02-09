
use crate::analyzer::error::{AnalyzerError, AnalyzerResult};
use surrealdb::sql::{Function, Kind};
use super::AnalyzerContext;

/// Analyze functions in the "type" namespace.
///
/// Conversion functions:
///   • type::array(any) -> array<any>
///   • type::bool(any) -> bool
///   • type::bytes(any) -> bytes
///   • type::datetime(any) -> datetime
///   • type::decimal(any) -> number
///   • type::duration(any) -> duration
///   • type::field(any) -> any
///   • type::fields(any) -> any
///   • type::float(any) -> number
///   • type::int(any) -> number
///   • type::number(any) -> number
///   • type::point(any) -> point
///   • type::range(any) -> range<record>
///   • type::record(string [, string]) -> record
///   • type::string(any) -> string
///   • type::table(any) -> string
///   • type::thing(any, any) -> record
///   • type::uuid(any) -> uuid
///
/// Type-checking functions:
///   • type::is::<subtype>(any) -> bool
/// where <subtype> is one of: array, bool, bytes, collection, datetime, decimal,
/// duration, float, geometry, int, line, none, null, multiline, multipoint,
/// multipolygon, number, object, point, polygon, record (optionally 1–2 args),
/// string, uuid.
pub(super) fn analyze_type(ctx: &AnalyzerContext, func: &Function) -> AnalyzerResult<Kind> {
    let name = func.name().ok_or(AnalyzerError::UnexpectedSyntax)?;
    let segments: Vec<&str> = name.split("::").collect();

    // Must be in the "type" namespace.
    if segments.is_empty() || segments[0] != "type" {
        return Err(AnalyzerError::FunctionNotFound(name.to_string()));
    }
    if segments.len() < 2 {
        return Err(AnalyzerError::UnexpectedSyntax);
    }

    match segments[1] {
        // Conversion functions – require exactly one argument.
        "array" => {
            if func.args().len() != 1 {
                Err(AnalyzerError::UnexpectedSyntax)
            } else {
                // Return an array of Any.
                Ok(Kind::Array(Box::new(Kind::Any), None))
            }
        },
        "bool" => {
            if func.args().len() != 1 {
                Err(AnalyzerError::UnexpectedSyntax)
            } else {
                Ok(Kind::Bool)
            }
        },
        "bytes" => {
            if func.args().len() != 1 {
                Err(AnalyzerError::UnexpectedSyntax)
            } else {
                Ok(Kind::Bytes)
            }
        },
        "datetime" => {
            if func.args().len() != 1 {
                Err(AnalyzerError::UnexpectedSyntax)
            } else {
                Ok(Kind::Datetime)
            }
        },
        "decimal" => {
            if func.args().len() != 1 {
                Err(AnalyzerError::UnexpectedSyntax)
            } else {
                // Represent decimals as numbers.
                Ok(Kind::Number)
            }
        },
        "duration" => {
            if func.args().len() != 1 {
                Err(AnalyzerError::UnexpectedSyntax)
            } else {
                Ok(Kind::Duration)
            }
        },
        "field" | "fields" => {
            if func.args().len() != 1 {
                Err(AnalyzerError::UnexpectedSyntax)
            } else {
                // Projection functions return an unconstrained type.
                Ok(Kind::Any)
            }
        },
        "float" => {
            if func.args().len() != 1 {
                Err(AnalyzerError::UnexpectedSyntax)
            } else {
                Ok(Kind::Number)
            }
        },
        "int" | "number" => {
            if func.args().len() != 1 {
                Err(AnalyzerError::UnexpectedSyntax)
            } else {
                Ok(Kind::Number)
            }
        },
        "point" => {
            if func.args().len() != 1 {
                Err(AnalyzerError::UnexpectedSyntax)
            } else {
                Ok(Kind::Point)
            }
        },
        "range" => {
            if func.args().len() != 1 {
                Err(AnalyzerError::UnexpectedSyntax)
            } else {
                // Returns a range of records.
                // We explicitly construct a Kind value for record before boxing it.
                Ok(Kind::Range)
            }
        },
        "record" => {
            let arg_count = func.args().len();
            if arg_count < 1 || arg_count > 2 {
                Err(AnalyzerError::UnexpectedSyntax)
            } else {
                // Return a record type.
                Ok(Kind::Record(vec![]))
            }
        },
        "string" => {
            if func.args().len() != 1 {
                Err(AnalyzerError::UnexpectedSyntax)
            } else {
                Ok(Kind::String)
            }
        },
        "table" => {
            if func.args().len() != 1 {
                Err(AnalyzerError::UnexpectedSyntax)
            } else {
                // A table is represented as a string.
                Ok(Kind::String)
            }
        },
        "thing" => {
            if func.args().len() != 2 {
                Err(AnalyzerError::UnexpectedSyntax)
            } else {
                // Return a record pointer.
                Ok(Kind::Record(vec![]))
            }
        },
        "uuid" => {
            if func.args().len() != 1 {
                Err(AnalyzerError::UnexpectedSyntax)
            } else {
                Ok(Kind::Uuid)
            }
        },
        "is" => {
            // type::is::<subtype>(any) -> bool.
            if segments.len() < 3 {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
            let subtype = segments[2];
            match subtype {
                // All these expect exactly one argument.
                "array" | "bool" | "bytes" | "collection" | "datetime" |
                "decimal" | "duration" | "float" | "geometry" | "int" |
                "line" | "none" | "null" | "multiline" | "multipoint" |
                "multipolygon" | "number" | "object" | "point" |
                "polygon" | "string" | "uuid" => {
                    if func.args().len() != 1 {
                        Err(AnalyzerError::UnexpectedSyntax)
                    } else {
                        Ok(Kind::Bool)
                    }
                },
                // The "record" type check can accept one or two arguments.
                "record" => {
                    let len = func.args().len();
                    if len < 1 || len > 2 {
                        Err(AnalyzerError::UnexpectedSyntax)
                    } else {
                        Ok(Kind::Bool)
                    }
                },
                _ => Err(AnalyzerError::FunctionNotFound(format!("type::is::{}", subtype))),
            }
        },
        other => Err(AnalyzerError::FunctionNotFound(format!("type::{}", other))),
    }
}
