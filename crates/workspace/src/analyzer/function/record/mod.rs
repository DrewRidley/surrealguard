//! `record` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod exists;
pub mod id;
pub mod is_edge;
pub mod refs;
pub mod table;
pub mod tb;

pub fn analyze_record_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "record::exists" => exists::analyze_record_exists(ctx, call, args),
        "record::id" => id::analyze_record_id(ctx, call, args),
        "record::is_edge" => is_edge::analyze_record_is_edge(ctx, call, args),
        "record::refs" => refs::analyze_record_refs(ctx, call, args),
        "record::table" => table::analyze_record_table(ctx, call, args),
        "record::tb" => tb::analyze_record_tb(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
