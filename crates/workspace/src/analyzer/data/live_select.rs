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
    // A live query subscribes to a table. A record-id source is checked
    // against its table too — the id is wrong for a different reason, which
    // the live contract reports, and the table it names still has to exist.
    for from in &stmt.from {
        match &from.node {
            ast::Expr::Table(table) => {
                crate::analyzer::data::check_table_reference(ctx, &table.node, table.span);
            }
            ast::Expr::RecordId { table, .. } => {
                crate::analyzer::data::check_table_reference(ctx, &table.node, from.span);
            }
            _ => {}
        }
    }
    // What a live query may be, on top of what its table must be.
    crate::analyzer::data::live_contract::check_live_select_statement(ctx, stmt);
    Kind::Uuid
}
