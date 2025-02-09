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


pub fn analyze_function(ctx: &AnalyzerContext, func: &Function) -> AnalyzerResult<Kind> {
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

        Some(_) | None => Err(AnalyzerError::FunctionNotFound(name.to_string())),
    }
}
