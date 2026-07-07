//! `KILL statement` analysis.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_kill(ctx: &mut AnalysisContext<'_>, stmt: &ast::KillStmt) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
