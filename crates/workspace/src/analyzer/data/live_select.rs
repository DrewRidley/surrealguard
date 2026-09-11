//! `LIVE SELECT` statement analysis.
//!
//! The statement's immediate response is the live query's subscription id —
//! a UUID. Its projection, `WHERE` and `FETCH` clauses are the same positions
//! a `SELECT` has, checked under the same contracts by the SELECT analyzer
//! (table and field references, condition types, FETCH targets); only the
//! response differs (the notification rows matter to host integrations, not
//! to the statement's own type). What a *live* query may additionally be —
//! one table, never a record id, and a `DIFF` that honours what stands beside
//! it — is the live contract, checked once here and nowhere else
//! (4009/4027, [`super::live_contract`]).

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_live_select(
    ctx: &mut AnalysisContext<'_>,
    stmt: &ast::LiveSelectStmt,
) -> Kind {
    if stmt.diff.is_some() {
        // `LIVE SELECT DIFF` projects nothing of its own; only the source has
        // to exist. A record-id source is checked against its table too — the
        // id is wrong for a different reason, which the live contract
        // reports, and the table it names still has to exist.
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
    } else {
        // Every clause a live query has, a SELECT has: check them as one, so
        // the field, condition and FETCH contracts have a single owner. A
        // `DIFF` that lost its leading position is the live contract's (4027)
        // and is not also a missing field.
        let projections = stmt
            .projections
            .iter()
            .filter(|projection| {
                !matches!(projection, ast::Projection::Expr { expr, alias: None }
                    if crate::analyzer::data::live_contract::is_bare_diff_path(&expr.node))
            })
            .cloned()
            .collect();
        let select = ast::SelectStmt {
            only: false,
            value: stmt.value,
            projections,
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
        // The read-shape lints do not apply to a subscription: a live query
        // has no LIMIT to add (7014), and `*` is how it asks for the whole
        // changed record rather than an over-fetch of a result set (7015).
        let before = ctx.diagnostics().len();
        crate::analyzer::data::select::analyze_select(ctx, &select);
        ctx.retain_since(before, |finding| {
            !matches!(finding.code().number(), 7014 | 7015)
        });
    }
    // What a live query may be, on top of what its table must be.
    crate::analyzer::data::live_contract::check_live_select_statement(ctx, stmt);
    Kind::Uuid
}
