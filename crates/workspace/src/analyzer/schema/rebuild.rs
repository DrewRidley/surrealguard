//! `REBUILD statement` analysis.
//!
//! A rebuild names a known table (1001) and an index that exists on it
//! (1012).

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_rebuild(ctx: &mut AnalysisContext<'_>, stmt: &ast::RebuildStmt) -> Kind {
    if let Some(table) = &stmt.table {
        crate::analyzer::data::check_table_reference(ctx, &table.node, table.span);
    }
    if let (Some(index), Some(table)) = (&stmt.index, &stmt.table) {
        crate::analyzer::schema::define::index::check_index_target(
            ctx,
            &index.node,
            index.span,
            &table.node,
            table.span,
            "REBUILD",
        );
    }
    Kind::None
}
