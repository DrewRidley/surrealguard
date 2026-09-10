//! `DEFINE INDEX` analysis.
//!
//! An index must target a known table (1001) over fields that table declares
//! (1002), is defined once (1022), and two indexes over the same field set do
//! the same work twice (1029). The `1012` index-target check for `REBUILD`/`REMOVE INDEX` lives
//! here too, since it is the same catalog reference from the other side.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_define_index(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineIndex) -> Kind {
    let refs = crate::schema::index_field_refs(stmt, ctx.source());
    let source = ctx.source().clone();

    let Some(table) = ctx.schema().table(&stmt.table.node) else {
        let finding = surrealguard_diagnostics::catalog::finding(
            SourceSpan::new(source, stmt.table.span),
            1001,
            format!(
                "index `{}` is defined on `{}`, which is not a defined table",
                stmt.name.node, stmt.table.node
            ),
        );
        let finding = crate::analyzer::data::with_table_suggestion(finding, ctx, &stmt.table.node);
        ctx.emit(finding);
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
    let this_kind = crate::schema::index_def_from_ast(stmt, ctx.source()).kind;
    let duplicate = (!paths.is_empty())
        .then(|| {
            table.indexes.values().find(|other| {
                let mut other_paths = other.field_paths();
                other_paths.sort();
                // A UNIQUE index and a plain index over the same fields do
                // different work (one enforces a constraint, the other only
                // speeds lookups), so they are not redundant. Two indexes are
                // duplicates only when their backing kind matches too.
                other.name != stmt.name.node && other.kind == this_kind && other_paths == paths
            })
        })
        .flatten()
        .map(|other| other.name.clone());

    let table_name_span = table.name_span.clone();
    let field_keys: Vec<String> = table.fields.keys().cloned().collect();
    let existing = (!stmt.overwrite && !stmt.if_not_exists)
        .then(|| table.indexes.get(&stmt.name.node))
        .flatten()
        .map(|existing| existing.name_span.clone());

    if let Some(existing) = existing {
        super::emit_duplicate_definition(
            ctx,
            stmt.name.span,
            &format!("`{}` on `{}`", stmt.name.node, stmt.table.node),
            &format!(
                "DEFINE INDEX OVERWRITE {} ON {}",
                stmt.name.node, stmt.table.node
            ),
            existing,
        );
    }

    for (text, span) in unknown_fields {
        let mut finding = surrealguard_diagnostics::catalog::finding(
            span,
            1002,
            format!(
                "`{}` has no field `{text}` (used by index `{}`)",
                stmt.table.node, stmt.name.node
            ),
        );
        if let Some(nearest) = crate::suggest::closest(&text, field_keys.iter().map(String::as_str))
        {
            finding = finding.with_help(format!("did you mean `{nearest}`?"));
        }
        finding = finding.with_related(
            table_name_span.clone(),
            format!("`{}` is defined here", stmt.table.node),
        );
        ctx.emit(finding);
    }

    if let Some(existing) = duplicate {
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                SourceSpan::new(ctx.source().clone(), stmt.name.span),
                1029,
                format!(
                    "index `{}` covers the same fields as `{existing}`",
                    stmt.name.node
                ),
            )
            .with_help("drop one — the duplicate index adds write cost without benefit"),
        );
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
            format!("{statement} targets `{table}`, which is not a defined table"),
        )),
        Some(table_def) if !table_def.indexes.contains_key(index) => {
            ctx.emit(surrealguard_diagnostics::catalog::finding(
                SourceSpan::new(source, index_span),
                1012,
                format!("`{table}` has no index `{index}` ({statement})"),
            ));
        }
        Some(_) => {}
    }
}
