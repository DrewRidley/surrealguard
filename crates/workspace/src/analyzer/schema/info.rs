//! `INFO FOR` statement analysis: returns the catalog description object
//! for the requested level (root/namespace/database/table).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_info(ctx: &mut AnalysisContext<'_>, stmt: &ast::InfoStmt) -> Kind {
    let _ = (ctx, stmt);
    Kind::Object
}
