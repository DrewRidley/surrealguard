//! `KILL` statement analysis.
//!
//! The statement returns nothing; its one invariant is that the argument
//! must be a live-query id (a uuid) — anything else is 2020.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_kill(ctx: &mut AnalysisContext<'_>, stmt: &ast::KillStmt) -> Kind {
    if let Some(id) = &stmt.id {
        let kind = crate::analyzer::expression::infer::infer_expression_fact(id, ctx).kind;
        if let Some(kind) = kind {
            if !matches!(kind, Kind::Uuid | Kind::Any) {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), id.span);
                ctx.emit(surrealguard_diagnostics::catalog::finding(
                    span,
                    2020,
                    format!("KILL expects a live-query uuid, found `{kind}`"),
                ));
            }
        }
    }
    Kind::None
}
