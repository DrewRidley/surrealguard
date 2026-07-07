//! Data statement analyzers.
//!
//! These modules model user data operations and their result shapes.

pub mod create;
pub mod delete;
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
    ctx.emit(surrealguard_diagnostics::catalog::finding(
        span,
        1001,
        format!("unknown table `{name}`"),
    ));
    false
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
    ctx.emit(surrealguard_diagnostics::catalog::finding(
        span,
        code,
        format!(
            "unknown field `{}` on table `{}`",
            segments.join("."),
            table.name
        ),
    ));
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
