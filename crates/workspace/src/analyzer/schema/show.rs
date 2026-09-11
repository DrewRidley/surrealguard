//! `SHOW CHANGES` statement analysis: returns the change feed — an array of
//! changeset objects.
//!
//! `SINCE` is mandatory in the engine but optional in the grammar, so a
//! statement that omits it still parses and is reported here (2021) at the
//! statement rather than collapsing the whole source into a generic syntax
//! error. 3.2.3 answers `SHOW CHANGES FOR TABLE person;` with "Unexpected
//! token `;`, expected SINCE", and the `LIMIT` form the same way — `LIMIT`
//! does not stand in for `SINCE`.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_show(ctx: &mut AnalysisContext<'_>, stmt: &ast::ShowStmt) -> Kind {
    if let Some(table) = &stmt.table {
        if crate::analyzer::data::check_table_reference(ctx, &table.node, table.span) {
            let has_changefeed = ctx
                .schema()
                .tables
                .get(&table.node)
                .is_some_and(|def| def.changefeed);
            if !has_changefeed {
                let span = surrealql_analyzer_syntax::span::SourceSpan::new(
                    ctx.source().clone(),
                    table.span,
                );
                ctx.emit(
                    surrealql_analyzer_diagnostics::catalog::finding(
                        span,
                        4021,
                        format!(
                            "`{}` has no CHANGEFEED, so SHOW CHANGES reads nothing",
                            table.node
                        ),
                    )
                    .with_help("add `CHANGEFEED <duration>` to the table's `DEFINE TABLE`"),
                );
            }
        }
    }
    // SINCE takes a versionstamp (number) or a datetime string (2021) — and
    // it is not optional.
    match &stmt.since {
        None => {
            let span =
                surrealql_analyzer_syntax::span::SourceSpan::new(ctx.source().clone(), stmt.span);
            ctx.emit(
                surrealql_analyzer_diagnostics::catalog::finding(
                    span,
                    2021,
                    "SHOW CHANGES needs a SINCE: there is no default start point".to_string(),
                )
                .with_help(
                    "add `SINCE <versionstamp>` or `SINCE <datetime>` — SurrealDB answers \
                     a SHOW CHANGES without it with \"Unexpected token `;`, expected SINCE\"",
                ),
            );
        }
        Some(since) => {
            if let ast::Expr::Literal(ast::Literal::String(text)) = &since.node {
                use std::str::FromStr;
                if surrealdb_types::Datetime::from_str(text).is_err() {
                    let span = surrealql_analyzer_syntax::span::SourceSpan::new(
                        ctx.source().clone(),
                        since.span,
                    );
                    ctx.emit(surrealql_analyzer_diagnostics::catalog::finding(
                        span,
                        2021,
                        format!(
                            "SHOW SINCE needs a versionstamp or datetime, but `{text}` is neither"
                        ),
                    ));
                }
            }
        }
    }
    Kind::Array(Box::new(Kind::Object), None)
}
