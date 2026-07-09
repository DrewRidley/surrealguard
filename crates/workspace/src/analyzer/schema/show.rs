//! `SHOW CHANGES` statement analysis: returns the change feed — an array of
//! changeset objects.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_show(ctx: &mut AnalysisContext<'_>, stmt: &ast::ShowStmt) -> Kind {
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
                ctx.emit(surrealguard_diagnostics::catalog::finding(
                    span,
                    4021,
                    format!(
                        "`{}` has no CHANGEFEED; SHOW CHANGES has nothing to read",
                        table.node
                    ),
                ));
            }
        }
    }
    Kind::Array(Box::new(Kind::Object), None)
}
