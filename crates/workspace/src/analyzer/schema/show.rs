//! `SHOW CHANGES` statement analysis: returns the change feed — an array of
//! changeset objects.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

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
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), table.span);
                ctx.emit(
                    surrealguard_diagnostics::catalog::finding(
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
    // SINCE takes a versionstamp (number) or a datetime string (2021).
    if let Some(since) = &stmt.since {
        if let ast::Expr::Literal(ast::Literal::String(text)) = &since.node {
            use std::str::FromStr;
            if surrealdb_types::Datetime::from_str(text).is_err() {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), since.span);
                ctx.emit(surrealguard_diagnostics::catalog::finding(
                    span,
                    2021,
                    format!("SHOW SINCE needs a versionstamp or datetime, but `{text}` is neither"),
                ));
            }
        }
    }
    Kind::Array(Box::new(Kind::Object), None)
}
