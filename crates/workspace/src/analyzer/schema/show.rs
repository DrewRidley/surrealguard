//! `SHOW CHANGES` statement analysis: returns the change feed — an array of
//! changeset objects.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_show(ctx: &mut AnalysisContext<'_>, stmt: &ast::ShowStmt) -> Kind {
    let _ = (ctx, stmt);
    Kind::Array(Box::new(Kind::Object), None)
}
