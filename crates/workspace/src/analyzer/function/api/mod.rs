//! `api` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod invoke;
pub mod timeout;

pub fn analyze_api_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "api::invoke" => invoke::analyze_api_invoke(ctx, call, args),
        "api::timeout" => timeout::analyze_api_timeout(ctx, call, args),
        _ => Kind::Any,
    }
}
