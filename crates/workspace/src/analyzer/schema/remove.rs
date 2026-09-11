//! `REMOVE statement` analysis.
//!
//! A removal names something that exists: a `TABLE`/`FIELD`/`EVENT`/
//! `FUNCTION`/`PARAM`/`ANALYZER` target must be defined (1021), and a
//! `REMOVE INDEX` target must exist on its table (1012, the same
//! schema-object reference `REBUILD INDEX` makes). The catalog mutation
//! itself is applied by the pipeline after this runs, so the schema here
//! still shows the pre-removal state.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;
use surrealql_analyzer_syntax::span::{ByteRange, SourceSpan};

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_remove(ctx: &mut AnalysisContext<'_>, stmt: &ast::RemoveStmt) -> Kind {
    match &stmt.target {
        ast::RemoveTarget::Table(table) => {
            if !ctx.schema().tables.contains_key(&table.node) {
                ctx.emit(
                    surrealql_analyzer_diagnostics::catalog::finding(
                        SourceSpan::new(ctx.source().clone(), table.span),
                        1021,
                        format!(
                            "REMOVE TABLE `{}` targets a table that doesn't exist",
                            table.node
                        ),
                    )
                    .with_help("nothing to remove — no such table"),
                );
            }
        }
        ast::RemoveTarget::Field { field, table } => {
            let field_text = crate::schema::idiom_field_path(&field.node).join(".");
            match ctx.schema().tables.get(&table.node) {
                None => ctx.emit(surrealql_analyzer_diagnostics::catalog::finding(
                    SourceSpan::new(ctx.source().clone(), table.span),
                    1021,
                    format!(
                        "REMOVE FIELD `{field_text}` targets `{}`, which is not a defined table",
                        table.node
                    ),
                )),
                Some(table_def) if !table_def.fields.contains_key(&field_text) => {
                    let related = table_def.name_span.clone();
                    ctx.emit(
                        surrealql_analyzer_diagnostics::catalog::finding(
                            SourceSpan::new(ctx.source().clone(), field.span),
                            1021,
                            format!("`{}` has no field `{field_text}` to remove", table.node),
                        )
                        .with_related(related, format!("`{}` is defined here", table.node)),
                    );
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
        ast::RemoveTarget::Event { event, table } => match ctx.schema().tables.get(&table.node) {
            None => ctx.emit(surrealql_analyzer_diagnostics::catalog::finding(
                SourceSpan::new(ctx.source().clone(), table.span),
                1021,
                format!(
                    "REMOVE EVENT `{}` targets `{}`, which is not a defined table",
                    event.node, table.node
                ),
            )),
            Some(table_def) if !table_def.events.contains_key(&event.node) => {
                let related = table_def.name_span.clone();
                ctx.emit(
                    surrealql_analyzer_diagnostics::catalog::finding(
                        SourceSpan::new(ctx.source().clone(), event.span),
                        1021,
                        format!("`{}` has no event `{}` to remove", table.node, event.node),
                    )
                    .with_related(related, format!("`{}` is defined here", table.node)),
                );
            }
            Some(_) => {}
        },
        ast::RemoveTarget::Function(name) => {
            if ctx.schema().function(&name.node).is_none() {
                emit_missing(ctx, name.span, "FUNCTION", &name.node, "function");
            }
        }
        ast::RemoveTarget::Param(name) => {
            if ctx.schema().param(&name.node).is_none() {
                emit_missing(ctx, name.span, "PARAM", &format!("${}", name.node), "param");
            }
        }
        ast::RemoveTarget::Analyzer(name) => {
            if ctx.schema().analyzer(&name.node).is_none() {
                emit_missing(ctx, name.span, "ANALYZER", &name.node, "analyzer");
            }
        }
        ast::RemoveTarget::Other(_) => {}
    }
    Kind::None
}

/// The 1021 finding for a database-level object (`FUNCTION`/`PARAM`/
/// `ANALYZER`) that was never defined, in the `REMOVE TABLE` wording family.
fn emit_missing(
    ctx: &mut AnalysisContext<'_>,
    span: ByteRange,
    keyword: &str,
    display: &str,
    noun: &str,
) {
    ctx.emit(
        surrealql_analyzer_diagnostics::catalog::finding(
            SourceSpan::new(ctx.source().clone(), span),
            1021,
            format!("REMOVE {keyword} `{display}` targets a {noun} that doesn't exist"),
        )
        .with_help(format!("nothing to remove — no such {noun}")),
    );
}
