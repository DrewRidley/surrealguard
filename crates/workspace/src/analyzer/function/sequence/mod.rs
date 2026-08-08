//! `sequence` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod nextval;

pub(crate) fn analyze_sequence_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "sequence::nextval" => nextval::analyze_sequence_nextval(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
