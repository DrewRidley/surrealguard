//! `ALTER statement` analysis.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_alter(ctx: &mut AnalysisContext<'_>, stmt: &ast::AlterStmt) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
