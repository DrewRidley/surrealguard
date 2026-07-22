//! `REMOVE statement` analysis.
//!
//! A removal names something that exists: a `TABLE`/`FIELD` target must be
//! defined (1021), and a `REMOVE INDEX` target must exist on its table
//! (1012). The catalog mutation itself is applied by the pipeline after this
//! runs, so the schema here still shows the pre-removal state.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;
use surrealguard_syntax::span::SourceSpan;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_remove(ctx: &mut AnalysisContext<'_>, stmt: &ast::RemoveStmt) -> Kind {
    match &stmt.target {
        ast::RemoveTarget::Table(table) => {
            if !ctx.schema().tables.contains_key(&table.node) {
                ctx.emit(surrealguard_diagnostics::catalog::finding(
                    SourceSpan::new(ctx.source().clone(), table.span),
                    1021,
                    format!("REMOVE TABLE targets unknown table `{}`", table.node),
                ));
            }
        }
        ast::RemoveTarget::Field { field, table } => {
            let field_text = crate::schema::idiom_field_path(&field.node).join(".");
            match ctx.schema().tables.get(&table.node) {
                None => ctx.emit(surrealguard_diagnostics::catalog::finding(
                    SourceSpan::new(ctx.source().clone(), table.span),
                    1021,
                    format!(
                        "REMOVE FIELD `{field_text}` targets unknown table `{}`",
                        table.node
                    ),
                )),
                Some(table_def) if !table_def.fields.contains_key(&field_text) => {
                    ctx.emit(surrealguard_diagnostics::catalog::finding(
                        SourceSpan::new(ctx.source().clone(), field.span),
                        1021,
                        format!(
                            "REMOVE FIELD targets unknown field `{field_text}` on table `{}`",
                            table.node
                        ),
                    ));
                }
                Some(_) => {}
            }
        }
        ast::RemoveTarget::Index { index, table } => {
            crate::analyzer::schema::define::index::check_index_target(
                ctx,
                &index.node,
                index.span,
                &table.node,
                table.span,
                "REMOVE",
            );
        }
        ast::RemoveTarget::Other(_) => {}
    }
    Kind::None
}
