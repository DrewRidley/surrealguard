//! `ALTER statement` analysis.
//!
//! `ALTER TABLE` names a known table (1001). Its effect — `SCHEMAFULL`/
//! `SCHEMALESS` and `DROP` rewriting the table's flags — is applied to the
//! catalog by the pipeline after this runs, so later field checks and reads
//! see the altered table.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_alter(ctx: &mut AnalysisContext<'_>, stmt: &ast::AlterStmt) -> Kind {
    if let Some(table) = &stmt.table {
        crate::analyzer::data::check_table_reference(ctx, &table.node, table.span);
    }
    Kind::None
}
