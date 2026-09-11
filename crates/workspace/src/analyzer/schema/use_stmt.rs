//! `USE statement` analysis.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_use(ctx: &mut AnalysisContext<'_>, stmt: &ast::UseStmt) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
