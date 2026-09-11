//! `RELATE` statement analysis.
//!
//! Owns RELATE-specific analysis: the target table is the *edge* named in
//! the traversal (`from->edge->to`), and the three endpoints are separate
//! spanned positions so edge-endpoint invariants can point at each.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::data::mutation;

pub(crate) fn analyze_relate(ctx: &mut AnalysisContext<'_>, stmt: &ast::RelateStmt) -> Kind {
    relate_response_kind(stmt, ctx)
}

pub(crate) fn relate_response_kind(stmt: &ast::RelateStmt, ctx: &mut AnalysisContext<'_>) -> Kind {
    // Endpoint tables are checkable regardless of whether the edge
    // resolves.
    for endpoint in [stmt.from.as_ref(), stmt.to.as_ref()].into_iter().flatten() {
        if let Some(endpoint_table) = endpoint_table_name(endpoint) {
            if !ctx.schema().tables.contains_key(&endpoint_table) {
                let span = surrealql_analyzer_syntax::span::SourceSpan::new(
                    ctx.source().clone(),
                    endpoint.span,
                );
                let finding = surrealql_analyzer_diagnostics::catalog::finding(
                    span,
                    1001,
                    format!("`{endpoint_table}` is not a defined table"),
                );
                let finding =
                    crate::analyzer::data::with_table_suggestion(finding, ctx, &endpoint_table);
                ctx.emit(finding);
            }
        }
    }

    let table_hint = mutation::source_table_name(stmt.edge.as_ref());
    mutation::analyze_expression_positions_for(
        ctx,
        stmt.data.as_ref(),
        None,
        table_hint.as_deref(),
        true,
    );
    let Some(table_name) = mutation::source_table_name(stmt.edge.as_ref()) else {
        return Kind::Any;
    };
    let Some(table) = ctx.schema().tables.get(&table_name) else {
        if let Some(edge) = &stmt.edge {
            crate::analyzer::data::check_table_reference(ctx, &table_name, edge.span);
        }
        return Kind::Any;
    };

    check_relate_endpoints(ctx, stmt, &table_name);
    mutation::response_kind_for_target(stmt.only, stmt.ret.as_ref(), table, ctx)
}

/// RELATE's own invariants: the edge must be a relation table (3007), the
/// endpoints must name known tables (3008), and each endpoint must sit on
/// the side the relation declares for it (3006).
fn check_relate_endpoints(ctx: &mut AnalysisContext<'_>, stmt: &ast::RelateStmt, edge_name: &str) {
    let Some(relation) = ctx
        .schema()
        .tables
        .get(edge_name)
        .and_then(|table| table.relation.clone())
    else {
        if let Some(edge) = &stmt.edge {
            let span =
                surrealql_analyzer_syntax::span::SourceSpan::new(ctx.source().clone(), edge.span);
            ctx.emit(
                surrealql_analyzer_diagnostics::catalog::finding(
                    span,
                    3001,
                    format!("`{edge_name}` can't be used in RELATE — it is not a relation table"),
                )
                .with_help("only tables defined with `TYPE RELATION` can be related"),
            );
        }
        return;
    };

    // Compare the written shape against the declared one; endpoints that
    // are params/computed or unknown tables (3008 already fired) don't
    // participate.
    let known = |endpoint: Option<&ast::Spanned<ast::Expr>>| {
        let name = endpoint_table_name(endpoint?)?;
        ctx.schema().tables.contains_key(&name).then_some(name)
    };
    let from = known(stmt.from.as_ref());
    let to = known(stmt.to.as_ref());

    let from_ok = from
        .as_ref()
        .is_none_or(|name| relation.in_tables.contains(name));
    let to_ok = to
        .as_ref()
        .is_none_or(|name| relation.out_tables.contains(name));
    if from_ok && to_ok {
        return;
    }

    let Some(anchor) = stmt.edge.as_ref().or(stmt.from.as_ref()) else {
        return;
    };
    let written = format!(
        "`{}`->`{edge_name}`->`{}`",
        from.as_deref().unwrap_or("?"),
        to.as_deref().unwrap_or("?"),
    );
    let declared_at = ctx
        .schema()
        .tables
        .get(edge_name)
        .map(|table| table.name_span.clone());
    let span = surrealql_analyzer_syntax::span::SourceSpan::new(ctx.source().clone(), anchor.span);
    let mut finding = surrealql_analyzer_diagnostics::catalog::finding(
        span,
        3002,
        format!(
            "relation `{edge_name}` connects {}, but this RELATE writes {written}",
            crate::analyzer::data::graph::declared_shape(edge_name, &relation),
        ),
    );
    if let Some(declared_at) = declared_at {
        finding =
            finding.with_related(declared_at, format!("relation `{edge_name}` declared here"));
    }
    ctx.emit(finding);
}

fn endpoint_table_name(endpoint: &ast::Spanned<ast::Expr>) -> Option<String> {
    match &endpoint.node {
        ast::Expr::Table(name) => Some(name.node.clone()),
        ast::Expr::RecordId { table, .. } => Some(table.node.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SchemaIndex;
    use crate::statement_env::StatementEnv;
    use surrealql_analyzer_syntax::parse::parse_source;
    use surrealql_analyzer_syntax::source::SourceId;

    use crate::schema::extract_schema;

    fn analyze(schema: &SchemaIndex, query: &str) -> Kind {
        let parsed = parse_source(SourceId::new("query"), query).expect("query should parse");
        let ast::Statement::Relate(stmt) =
            surrealql_analyzer_syntax::lower::lower_first_statement(&parsed, "RelateStatement")
                .expect("relate statement exists")
                .node
        else {
            panic!("expected relate statement");
        };
        let env = StatementEnv::default();
        let mut diagnostics: Vec<surrealql_analyzer_diagnostics::Finding> = Vec::new();
        let mut ctx = AnalysisContext::scoped(
            schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
            env,
            None,
        );
        relate_response_kind(&stmt, &mut ctx)
    }

    #[test]
    fn infers_array_of_full_table_rows_by_default() {
        let schema_parsed = parse_source(
            SourceId::new("schema"),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD strength ON likes TYPE float;",
        )
        .expect("schema should parse");
        let schema = extract_schema(&[schema_parsed]).schema;

        let kind = analyze(&schema, "RELATE person:one->likes->post:one;");

        let Kind::Array(element, None) = kind else {
            panic!("expected unbounded array kind, got {kind:?}");
        };
        let Kind::Literal(surrealdb_types::KindLiteral::Object(fields)) = *element else {
            panic!("expected object literal element");
        };
        assert_eq!(fields["strength"], Kind::Float);
    }

    #[test]
    fn unknown_target_table_is_poison() {
        let schema = SchemaIndex::default();

        let kind = analyze(&schema, "RELATE person:one->ghost->post:one;");

        assert_eq!(kind, Kind::Any);
    }
}
