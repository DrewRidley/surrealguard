//! `vector` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod add;
pub mod angle;
pub mod cross;
pub mod distance_chebyshev;
pub mod distance_euclidean;
pub mod distance_hamming;
pub mod distance_knn;
pub mod distance_mahalanobis;
pub mod distance_manhattan;
pub mod distance_minkowski;
pub mod divide;
pub mod dot;
pub mod magnitude;
pub mod multiply;
pub mod normalize;
pub mod project;
pub mod scale;
pub mod similarity_cosine;
pub mod similarity_jaccard;
pub mod similarity_pearson;
pub mod similarity_spearman;
pub mod subtract;

pub(crate) fn analyze_vector_function(
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
        "vector::distance::chebyshev" => distance_chebyshev::analyze_vector_distance_chebyshev(ctx, call, args),
        "vector::distance::euclidean" => distance_euclidean::analyze_vector_distance_euclidean(ctx, call, args),
        "vector::distance::hamming" => distance_hamming::analyze_vector_distance_hamming(ctx, call, args),
        "vector::distance::knn" => distance_knn::analyze_vector_distance_knn(ctx, call, args),
        "vector::distance::mahalanobis" => distance_mahalanobis::analyze_vector_distance_mahalanobis(ctx, call, args),
        "vector::distance::manhattan" => distance_manhattan::analyze_vector_distance_manhattan(ctx, call, args),
        "vector::distance::minkowski" => distance_minkowski::analyze_vector_distance_minkowski(ctx, call, args),
        "vector::similarity::cosine" => similarity_cosine::analyze_vector_similarity_cosine(ctx, call, args),
        "vector::similarity::jaccard" => similarity_jaccard::analyze_vector_similarity_jaccard(ctx, call, args),
        "vector::similarity::pearson" => similarity_pearson::analyze_vector_similarity_pearson(ctx, call, args),
        "vector::similarity::spearman" => similarity_spearman::analyze_vector_similarity_spearman(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
