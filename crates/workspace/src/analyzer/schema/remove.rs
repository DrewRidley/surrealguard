//! `REMOVE statement` analysis.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_remove(ctx: &mut AnalysisContext<'_>, stmt: &ast::RemoveStmt) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
