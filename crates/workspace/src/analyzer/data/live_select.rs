//! `LIVE SELECT` statement analysis.
//!
//! The statement's immediate response is the live query's subscription id —
//! a UUID. The notification stream's row type is the underlying SELECT's
//! row type, which matters to host integrations rather than to the
//! statement's own response.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_live_select(
    ctx: &mut AnalysisContext<'_>,
    stmt: &ast::LiveSelectStmt,
) -> Kind {
    if let Some(table) = &stmt.table {
        crate::analyzer::data::check_table_reference(ctx, &table.node, table.span);
    }
    Kind::Uuid
}
