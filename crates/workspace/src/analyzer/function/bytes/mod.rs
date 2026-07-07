//! `bytes` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod len;

pub fn analyze_bytes_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "bytes::len" => len::analyze_bytes_len(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
