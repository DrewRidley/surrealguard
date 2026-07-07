//! `REBUILD statement` analysis.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_rebuild(ctx: &mut AnalysisContext<'_>, stmt: &ast::RebuildStmt) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
