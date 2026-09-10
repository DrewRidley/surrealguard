//! `DEFINE ANALYZER` analysis.
//!
//! A full-text pipeline is defined once (1022) and names known tokenizers
//! and filters, with valid filter arguments (1032/2035).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::schema::SchemaIndex;

pub(crate) fn analyze_define_analyzer(
    ctx: &mut AnalysisContext<'_>,
    stmt: &ast::DefineAnalyzer,
) -> Kind {
    if !stmt.overwrite && !stmt.if_not_exists {
        let existing = ctx
            .schema()
            .analyzer(&stmt.name.node)
            .map(|existing| existing.name_span.clone());
        if let Some(existing) = existing {
            super::emit_duplicate_definition(
                ctx,
                stmt.name.span,
                &format!("`{}`", stmt.name.node),
                &format!("DEFINE ANALYZER OVERWRITE {}", stmt.name.node),
                existing,
            );
        }
    }
    let analyzer = crate::schema::analyzer_def_from_ast(stmt, ctx.source());
    for finding in SchemaIndex::validate_analyzer(&analyzer) {
        ctx.emit(finding);
    }
    Kind::None
}
