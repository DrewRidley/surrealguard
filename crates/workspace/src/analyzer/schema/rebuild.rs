//! `REBUILD statement` analysis.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_rebuild(ctx: &mut AnalysisContext<'_>, stmt: &ast::RebuildStmt) -> Kind {
    if let Some(table) = &stmt.table {
        crate::analyzer::data::check_table_reference(ctx, &table.node, table.span);
    }
    Kind::None
}
