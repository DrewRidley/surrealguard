//! `BEGIN` statement analysis. Produces no value.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_begin(ctx: &mut AnalysisContext<'_>, stmt: &ast::BeginStmt) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
