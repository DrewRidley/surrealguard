//! `COMMIT` statement analysis. Produces no value.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_commit(ctx: &mut AnalysisContext<'_>, stmt: &ast::CommitStmt) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
