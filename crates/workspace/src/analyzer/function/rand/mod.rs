//! `rand` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod bool;
pub mod duration;
pub mod r#enum;
pub mod float;
pub mod id;
pub mod int;
pub mod string;
pub mod time;
pub mod ulid;
pub mod uuid;

pub(crate) fn analyze_rand_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "rand::bool" => bool::analyze_rand_bool(ctx, call, args),
        "rand::duration" => duration::analyze_rand_duration(ctx, call, args),
        "rand::enum" => r#enum::analyze_rand_enum(ctx, call, args),
        "rand::float" => float::analyze_rand_float(ctx, call, args),
        "rand::id" => id::analyze_rand_id(ctx, call, args),
        "rand::int" => int::analyze_rand_int(ctx, call, args),
        "rand::string" => string::analyze_rand_string(ctx, call, args),
        "rand::time" => time::analyze_rand_time(ctx, call, args),
        "rand::ulid" => ulid::analyze_rand_ulid(ctx, call, args),
        "rand::uuid" => uuid::analyze_rand_uuid(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
