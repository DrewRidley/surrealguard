//! `DEFINE TABLE` analysis.
//!
//! A table is defined once: redefining it without `OVERWRITE` is a
//! duplicate definition (1022).

use surrealdb_types::{Kind, Table};
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_define_table(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineTable) -> Kind {
    if !stmt.overwrite {
        if let Some(existing) = ctx.schema().table(&stmt.name.node) {
            let span =
                surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), stmt.name.span);
            let mut finding = surrealguard_diagnostics::catalog::finding(
                span,
                1022,
                format!(
                    "`{}` is already defined; this DEFINE silently replaces the earlier one",
                    stmt.name.node
                ),
            )
            .with_help(format!(
                "use `DEFINE TABLE OVERWRITE {}` to redefine it intentionally",
                stmt.name.node
            ));
            finding = finding.with_related(
                existing.name_span.clone(),
                format!("`{}` is defined here", stmt.name.node),
            );
            ctx.emit(finding);
        }
    }

    // Each `PERMISSIONS FOR <action> WHERE <expr>` predicate is evaluated
    // against a row of this table; `$value` is that record.
    let record = Kind::Record(vec![Table::from(stmt.name.node.as_str())]);
    super::permissions::analyze_permission_predicates(
        ctx,
        &stmt.name.node,
        record,
        &stmt.permissions,
    );

    Kind::None
}
