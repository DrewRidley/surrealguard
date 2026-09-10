//! `LIVE SELECT` statement analysis.
//!
//! The statement's immediate response is the live query's subscription id —
//! a UUID. Its projection, `WHERE` and `FETCH` clauses are the same positions
//! a `SELECT` has, checked under the same contracts by the SELECT analyzer;
//! only the response differs (the notification rows matter to host
//! integrations, not to the statement's own type).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_live_select(
    ctx: &mut AnalysisContext<'_>,
    stmt: &ast::LiveSelectStmt,
) -> Kind {
    if stmt.diff {
        // `LIVE SELECT DIFF` projects nothing of its own; only the source has
        // to exist.
        for source in &stmt.from {
            if let ast::Expr::Table(table) = &source.node {
                crate::analyzer::data::check_table_reference(ctx, &table.node, table.span);
            }
        }
        return Kind::Uuid;
    }
    let select = ast::SelectStmt {
        only: false,
        value: stmt.value,
        projections: stmt.projections.clone(),
        from: stmt.from.clone(),
        omit: Vec::new(),
        fetch: stmt.fetch.clone(),
        split: Vec::new(),
        where_clause: stmt.where_clause.clone(),
        group: None,
        order: None,
        limit: None,
        start: None,
        explain: None,
        timeout: None,
        parallel: None,
    };
    crate::analyzer::data::select::analyze_select(ctx, &select);
    Kind::Uuid
}
