//! `SHOW CHANGES` statement analysis: returns the change feed — an array of
//! changeset objects.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_show(ctx: &mut AnalysisContext<'_>, stmt: &ast::ShowStmt) -> Kind {
    if let Some(table) = &stmt.table {
        crate::analyzer::data::check_table_reference(ctx, &table.node, table.span);
    }
    Kind::Array(Box::new(Kind::Object), None)
}
