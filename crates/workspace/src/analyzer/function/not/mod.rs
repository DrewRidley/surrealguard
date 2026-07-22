//! `not` function-family analysis dispatch.

// One-file-per-function layout: this namespace has a single function
// sharing its name, which is intentional.
#![allow(clippy::module_inception)]

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod not;

pub(crate) fn analyze_not_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "not::not" => not::analyze_not_not(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
