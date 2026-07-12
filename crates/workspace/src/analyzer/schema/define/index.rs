//! `DEFINE INDEX` analysis.
//!
//! An index must target a known table (1001) over fields that table declares
//! (1002), and two indexes over the same field set do the same work twice
//! (1029). The `1012` index-target check for `REBUILD`/`REMOVE INDEX` lives
//! here too, since it is the same catalog reference from the other side.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use crate::analyzer::context::AnalysisContext;

pub fn analyze_define_index(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineIndex) -> Kind {
    let refs = crate::schema::index_field_refs(stmt, ctx.source());
    let source = ctx.source().clone();

    let Some(table) = ctx.schema().table(&stmt.table.node) else {
        ctx.emit(surrealguard_diagnostics::catalog::finding(
            SourceSpan::new(source, stmt.table.span),
            1001,
            format!(
                "index `{}` targets unknown table `{}`",
                stmt.name.node, stmt.table.node
            ),
        ));
        return Kind::None;
    };

    let mut unknown_fields = Vec::new();
    for (path, text, span) in &refs {
        if !crate::schema::index_field_path_exists_on_table(table, path) {
            unknown_fields.push((text.clone(), span.clone()));
        }
    }

    // Two indexes over the same field set do the same work twice (1029).
    let mut paths: Vec<String> = refs.iter().map(|(path, _, _)| path.join(".")).collect();
    paths.sort();
    let duplicate = (!paths.is_empty())
        .then(|| {
            table.indexes.values().find(|other| {
                let mut other_paths = other.field_paths();
                other_paths.sort();
                other.name != stmt.name.node && other_paths == paths
            })
        })
        .flatten()
        .map(|other| other.name.clone());

    for (text, span) in unknown_fields {
        ctx.emit(surrealguard_diagnostics::catalog::finding(
            span,
            1002,
            format!(
                "index `{}` references unknown field `{}` on table `{}`",
                stmt.name.node, text, stmt.table.node
            ),
        ));
    }

    if let Some(existing) = duplicate {
        ctx.emit(surrealguard_diagnostics::catalog::finding(
            SourceSpan::new(ctx.source().clone(), stmt.name.span),
            1029,
            format!(
                "index `{}` covers the same fields as `{}`",
                stmt.name.node, existing
            ),
        ));
    }

    Kind::None
}

/// The `REBUILD`/`REMOVE INDEX` reference contract (1012): the named index
/// must exist on the named table.
pub(crate) fn check_index_target(
    ctx: &mut AnalysisContext<'_>,
    index: &str,
    index_span: ByteRange,
    table: &str,
    table_span: ByteRange,
    statement: &str,
) {
    let source = ctx.source().clone();
    match ctx.schema().table(table) {
        None => ctx.emit(surrealguard_diagnostics::catalog::finding(
            SourceSpan::new(source, table_span),
            1012,
            format!("index `{index}` targets unknown table `{table}` in {statement} statement"),
        )),
        Some(table_def) if !table_def.indexes.contains_key(index) => {
            ctx.emit(surrealguard_diagnostics::catalog::finding(
                SourceSpan::new(source, index_span),
                1012,
                format!("unknown index `{index}` on table `{table}` in {statement} statement"),
            ));
        }
        Some(_) => {}
    }
}
