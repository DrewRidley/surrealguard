//! `KILL` statement analysis.
//!
//! The statement returns nothing; its one invariant is that the argument
//! must be a live-query id (a uuid) — anything else is 2020.
//!
//! A plain string parses here because the grammar is deliberately looser
//! than the engine on this one shape: 3.2.3 answers `KILL "…"` with
//! "Unexpected token `a strand`, expected a UUID or a parameter" and gives
//! up on the whole source, which tells an editor nothing about where the
//! problem is. Parsing it and reporting 2020 at the argument does.
//! Uuid-shaped makes no difference — the engine rejects
//! `KILL "018e0f3a-1234-7abc-8def-0123456789ab"` too; only `u'…'` or a
//! parameter is a live-query id.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_kill(ctx: &mut AnalysisContext<'_>, stmt: &ast::KillStmt) -> Kind {
    if let Some(id) = &stmt.id {
        let kind = crate::analyzer::expression::infer::infer_expression_fact(id, ctx).kind;
        if let Some(kind) = kind {
            if !matches!(kind, Kind::Uuid | Kind::Any) {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), id.span);
                ctx.emit(
                    surrealguard_diagnostics::catalog::finding(
                        span,
                        2020,
                        format!(
                            "KILL needs a live-query uuid, but this is a `{}`",
                            crate::render::render_offending(&kind, Some(&Kind::Uuid))
                        ),
                    )
                    .with_help(
                        "pass the uuid LIVE SELECT returned, as a `u'…'` literal or a \
                         parameter — SurrealDB answers anything else with \
                         \"Unexpected token `a strand`, expected a UUID or a parameter\"",
                    ),
                );
            }
        }
    }
    Kind::None
}
