//! `CANCEL` statement analysis. Produces no value.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_cancel(ctx: &mut AnalysisContext<'_>, stmt: &ast::CancelStmt) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
