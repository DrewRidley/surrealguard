//! `CANCEL` statement analysis. Produces no value.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_cancel(ctx: &mut AnalysisContext<'_>, stmt: &ast::CancelStmt) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
