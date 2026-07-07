//! `meta` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod id;
pub mod table;
pub mod tb;

pub fn analyze_meta_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "meta::id" => id::analyze_meta_id(ctx, call, args),
        "meta::table" => table::analyze_meta_table(ctx, call, args),
        "meta::tb" => tb::analyze_meta_tb(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
