//! `schema` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod table_exists;

pub(crate) fn analyze_schema_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "schema::table::exists" => table_exists::analyze_schema_table_exists(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
