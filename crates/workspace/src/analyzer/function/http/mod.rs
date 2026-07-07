//! `http` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod delete;
pub mod get;
pub mod head;
pub mod patch;
pub mod post;
pub mod put;

pub fn analyze_http_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "http::delete" => delete::analyze_http_delete(ctx, call, args),
        "http::get" => get::analyze_http_get(ctx, call, args),
        "http::head" => head::analyze_http_head(ctx, call, args),
        "http::patch" => patch::analyze_http_patch(ctx, call, args),
        "http::post" => post::analyze_http_post(ctx, call, args),
        "http::put" => put::analyze_http_put(ctx, call, args),
        _ => Kind::Any,
    }
}
