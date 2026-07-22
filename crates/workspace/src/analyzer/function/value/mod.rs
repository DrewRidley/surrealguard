//! `value` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod diff;
pub mod expect;
pub mod patch;

pub(crate) fn analyze_value_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "value::diff" => diff::analyze_value_diff(ctx, call, args),
        "value::expect" => expect::analyze_value_expect(ctx, call, args),
        "value::patch" => patch::analyze_value_patch(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
