//! `vector` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod add;
pub mod angle;
pub mod cross;
pub mod divide;
pub mod dot;
pub mod magnitude;
pub mod multiply;
pub mod normalize;
pub mod project;
pub mod scale;
pub mod subtract;

pub fn analyze_vector_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "vector::add" => add::analyze_vector_add(ctx, call, args),
        "vector::angle" => angle::analyze_vector_angle(ctx, call, args),
        "vector::cross" => cross::analyze_vector_cross(ctx, call, args),
        "vector::divide" => divide::analyze_vector_divide(ctx, call, args),
        "vector::dot" => dot::analyze_vector_dot(ctx, call, args),
        "vector::magnitude" => magnitude::analyze_vector_magnitude(ctx, call, args),
        "vector::multiply" => multiply::analyze_vector_multiply(ctx, call, args),
        "vector::normalize" => normalize::analyze_vector_normalize(ctx, call, args),
        "vector::project" => project::analyze_vector_project(ctx, call, args),
        "vector::scale" => scale::analyze_vector_scale(ctx, call, args),
        "vector::subtract" => subtract::analyze_vector_subtract(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
