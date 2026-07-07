//! `INFO FOR` statement analysis: returns the catalog description object
//! for the requested level (root/namespace/database/table).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_info(ctx: &mut AnalysisContext<'_>, stmt: &ast::InfoStmt) -> Kind {
    if let Some(table) = &stmt.table {
        crate::analyzer::data::check_table_reference(ctx, &table.node, table.span);
    }
    Kind::Object
}
