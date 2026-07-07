//! `search` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod analyze;
pub mod highlight;
pub mod linear;
pub mod offsets;
pub mod rrf;
pub mod score;

pub fn analyze_search_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "search::analyze" => analyze::analyze_search_analyze(ctx, call, args),
        "search::highlight" => highlight::analyze_search_highlight(ctx, call, args),
        "search::linear" => linear::analyze_search_linear(ctx, call, args),
        "search::offsets" => offsets::analyze_search_offsets(ctx, call, args),
        "search::rrf" => rrf::analyze_search_rrf(ctx, call, args),
        "search::score" => score::analyze_search_score(ctx, call, args),
        _ => Kind::Any,
    }
}
