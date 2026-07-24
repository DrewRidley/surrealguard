//! `count` function-family analysis dispatch.

// One-file-per-function layout: this namespace has a single function
// sharing its name, which is intentional.
#![allow(clippy::module_inception)]

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod count;

pub(crate) fn analyze_count_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        // Lowering normalizes only `::is::` paths; a bare `count()` keeps the
        // path `"count"` (namespace == function name), so accept it as an
        // alias for the canonical `count::count`.
        "count" | "count::count" => count::analyze_count_count(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
