//! `DEFINE TABLE` analysis.
//!
//! A table is defined once: redefining it without `OVERWRITE` or `IF NOT
//! EXISTS` is a duplicate definition (1022).

use surrealdb_types::{Kind, Table};
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_define_table(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineTable) -> Kind {
    // `IF NOT EXISTS` makes a redefinition a deliberate no-op, exactly as
    // `OVERWRITE` makes it a deliberate replacement.
    if !stmt.overwrite && !stmt.if_not_exists {
        if let Some(existing) = ctx.schema().table(&stmt.name.node) {
            let existing = existing.name_span.clone();
            super::emit_duplicate_definition(
                ctx,
                stmt.name.span,
                &format!("`{}`", stmt.name.node),
                &format!("DEFINE TABLE OVERWRITE {}", stmt.name.node),
                existing,
            );
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
