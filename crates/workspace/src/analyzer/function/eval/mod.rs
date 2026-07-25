//! `eval` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod gql;
pub mod surql;

pub(crate) fn analyze_eval_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "eval::gql" => gql::analyze_eval_gql(ctx, call, args),
        "eval::surql" => surql::analyze_eval_surql(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
