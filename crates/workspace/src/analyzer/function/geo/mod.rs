//! `geo` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod area;
pub mod bearing;
pub mod centroid;
pub mod distance;
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
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
