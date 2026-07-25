//! `api` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod invoke;
pub mod req_body;
pub mod res_body;
pub mod res_header;
pub mod res_headers;
pub mod res_status;
pub mod timeout;

pub(crate) fn analyze_api_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "api::invoke" => invoke::analyze_api_invoke(ctx, call, args),
        "api::req::body" => req_body::analyze_api_req_body(ctx, call, args),
        "api::res::body" => res_body::analyze_api_res_body(ctx, call, args),
        "api::res::header" => res_header::analyze_api_res_header(ctx, call, args),
        "api::res::headers" => res_headers::analyze_api_res_headers(ctx, call, args),
        "api::res::status" => res_status::analyze_api_res_status(ctx, call, args),
        "api::timeout" => timeout::analyze_api_timeout(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
