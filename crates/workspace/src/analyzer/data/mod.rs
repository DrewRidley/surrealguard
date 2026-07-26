//! Data statement analyzers.
//!
//! These modules model user data operations and their result shapes.

pub mod create;
pub mod delete;
pub mod graph;
pub mod insert;
pub mod kill;
pub mod live_select;
pub(crate) mod mutation;
pub mod relate;
pub mod select;
pub mod update;
pub mod upsert;

/// Emits 1001 when `name` is not a table in the schema, anchored on the
/// reference. Returns whether the table exists so callers can keep their
/// control flow.
pub(crate) fn check_table_reference(
    ctx: &mut crate::analyzer::context::AnalysisContext<'_>,
    name: &str,
    span: surrealguard_syntax::span::ByteRange,
) -> bool {
    if ctx.schema().tables.contains_key(name) {
        return true;
    }
    let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), span);
    let finding = surrealguard_diagnostics::catalog::finding(
        span,
        1001,
        format!("`{name}` is not a defined table"),
    );
    let finding = with_table_suggestion(finding, ctx, name);
    ctx.emit(finding);
    false
}

/// [`check_table_reference`] for a *reference target* rather than a read: 1001
/// fires only when `name` is defined nowhere in the workspace, before or after.
///
/// A `DEFINE FIELD tasks ON project COMPUTED <~task` names the far side of a
/// mutual reference — `task` must itself carry a `record<project> REFERENCE`
/// field — so one of the two tables is always written "later" than the other.
/// Asking the incrementally-built catalog made the finding depend on which file
/// happened to sort first, contradicting its own help text ("no `DEFINE TABLE
/// task` exists in the workspace"). Same reasoning as the declared-`record<T>`
/// target check in `schema::define::field::check_record_targets`.
pub(crate) fn check_table_defined_anywhere(
    ctx: &mut crate::analyzer::context::AnalysisContext<'_>,
    name: &str,
    span: surrealguard_syntax::span::ByteRange,
) -> bool {
    if ctx.table_defined_anywhere(name) {
        return true;
    }
    let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), span);
    let mut finding = surrealguard_diagnostics::catalog::finding(
        span,
        1001,
        format!("`{name}` is not a defined table"),
    );
    finding = match crate::suggest::closest(name, ctx.known_table_names().into_iter()) {
        Some(nearest) => finding.with_help(format!("did you mean `{nearest}`?")),
        None => finding.with_help(format!("no `DEFINE TABLE {name}` exists in the workspace")),
    };
    ctx.emit(finding);
    false
}

/// Appends the standard 1001 "did you mean `<closest>`?" help to a finding
/// about an unknown table `name` — or, when no near name exists, the
/// "no `DEFINE TABLE <name>` exists" note. Shared so every emit site that
/// reports a missing table offers the same suggestion.
pub(crate) fn with_table_suggestion(
    finding: surrealguard_diagnostics::Finding,
    ctx: &crate::analyzer::context::AnalysisContext<'_>,
    name: &str,
) -> surrealguard_diagnostics::Finding {
    match crate::suggest::closest(name, ctx.schema().tables.keys().map(String::as_str)) {
        Some(nearest) => finding.with_help(format!("did you mean `{nearest}`?")),
        None => finding.with_help(format!("no `DEFINE TABLE {name}` exists in the workspace")),
    }
}

/// Emits `code` when a plain field path does not resolve on `table`,
/// anchored on the path. Field checks fire only for tables with declared
/// fields (schemaless rows are open by design).
pub(crate) fn check_field_path(
    ctx: &mut crate::analyzer::context::AnalysisContext<'_>,
    table: &crate::schema::TableDef,
    segments: &[String],
    span: surrealguard_syntax::span::ByteRange,
    code: u16,
) {
    if table.fields.is_empty() || select::kind_for_path(table, segments).is_some() {
        return;
    }
    let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), span);
    let path = segments.join(".");
    let mut finding = surrealguard_diagnostics::catalog::finding(
        span,
        code,
        format!("`{}` has no field `{path}`", table.name),
    );
    if let Some(nearest) =
        crate::suggest::closest(&path, table.fields.keys().map(String::as_str))
    {
        finding = finding.with_help(format!("did you mean `{nearest}`?"));
    }
    finding = finding.with_related(
        table.name_span.clone(),
        format!("`{}` is defined here", table.name),
    );
    ctx.emit(finding);
}

/// Walks an expression for plain field-path references and checks each
/// against the row table with `code`. Graph idioms belong to the graph
/// family; parameters and subqueries resolve elsewhere.
pub(crate) fn check_expression_field_paths(
    ctx: &mut crate::analyzer::context::AnalysisContext<'_>,
    table: &crate::schema::TableDef,
    expr: &surrealguard_syntax::ast::Spanned<surrealguard_syntax::ast::Expr>,
    code: u16,
) {
    match &expr.node {
        surrealguard_syntax::ast::Expr::Idiom(idiom) => {
            if select::is_graph_projection_idiom(idiom) {
                return;
            }
            if let Some(segments) = crate::analyzer::expression::infer::plain_field_segments(idiom)
            {
                check_field_path(ctx, table, &segments, expr.span, code);
            }
        }
        surrealguard_syntax::ast::Expr::Binary { lhs, rhs, .. } => {
            check_expression_field_paths(ctx, table, lhs, code);
            check_expression_field_paths(ctx, table, rhs, code);
        }
        surrealguard_syntax::ast::Expr::Prefix { expr: inner, .. } => {
            check_expression_field_paths(ctx, table, inner, code);
        }
        surrealguard_syntax::ast::Expr::Array(elements) => {
            for element in elements {
                check_expression_field_paths(ctx, table, element, code);
            }
        }
        surrealguard_syntax::ast::Expr::Object(fields) => {
            for (_, value) in fields {
                check_expression_field_paths(ctx, table, value, code);
            }
        }
        surrealguard_syntax::ast::Expr::Call(call) => {
            for arg in &call.args {
                check_expression_field_paths(ctx, table, arg, code);
            }
        }
        surrealguard_syntax::ast::Expr::Cast { expr: inner, .. } => {
            check_expression_field_paths(ctx, table, inner, code);
        }
        _ => {}
    }
}
