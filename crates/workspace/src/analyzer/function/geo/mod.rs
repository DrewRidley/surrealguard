//! `geo` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod area;
pub mod bearing;
pub mod centroid;
pub mod distance;
pub mod hash_decode;
pub mod hash_encode;
pub mod is_valid;

pub(crate) fn analyze_geo_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "geo::area" => area::analyze_geo_area(ctx, call, args),
        "geo::bearing" => bearing::analyze_geo_bearing(ctx, call, args),
        "geo::centroid" => centroid::analyze_geo_centroid(ctx, call, args),
        "geo::distance" => distance::analyze_geo_distance(ctx, call, args),
        "geo::is_valid" => is_valid::analyze_geo_is_valid(ctx, call, args),
        "geo::hash::decode" => hash_decode::analyze_geo_hash_decode(ctx, call, args),
        "geo::hash::encode" => hash_encode::analyze_geo_hash_encode(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
