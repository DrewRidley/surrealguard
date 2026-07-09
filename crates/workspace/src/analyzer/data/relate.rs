//! `RELATE` statement analysis.
//!
//! Owns RELATE-specific analysis: the target table is the *edge* named in
//! the traversal (`from->edge->to`), and the three endpoints are separate
//! spanned positions so edge-endpoint invariants can point at each.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::data::mutation;

pub fn analyze_relate(ctx: &mut AnalysisContext<'_>, stmt: &ast::RelateStmt) -> Kind {
    relate_response_kind(stmt, ctx)
}

pub(crate) fn relate_response_kind(stmt: &ast::RelateStmt, ctx: &mut AnalysisContext<'_>) -> Kind {
    // Endpoint tables are checkable regardless of whether the edge
    // resolves.
    for endpoint in [stmt.from.as_ref(), stmt.to.as_ref()].into_iter().flatten() {
        if let Some(endpoint_table) = endpoint_table_name(endpoint) {
            if !ctx.schema().tables.contains_key(&endpoint_table) {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), endpoint.span);
                ctx.emit(surrealguard_diagnostics::catalog::finding(
                    span,
                    3008,
                    format!("unknown table `{endpoint_table}` in RELATE endpoint"),
                ));
            }
        }
    }

    let table_hint = mutation::source_table_name(stmt.edge.as_ref());
    mutation::analyze_expression_positions(ctx, stmt.data.as_ref(), None, table_hint.as_deref());
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
            let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), edge.span);
            ctx.emit(surrealguard_diagnostics::catalog::finding(
                span,
                3007,
                format!("`{edge_name}` is not a relation table"),
            ));
        }
        return;
    };

    for (endpoint, side, tables) in [
        (stmt.from.as_ref(), "in", &relation.in_tables),
        (stmt.to.as_ref(), "out", &relation.out_tables),
    ] {
        let Some(endpoint) = endpoint else {
            continue;
        };
        let Some(endpoint_table) = endpoint_table_name(endpoint) else {
            // Params and computed endpoints resolve at runtime.
            continue;
        };
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), endpoint.span);
        if !ctx.schema().tables.contains_key(&endpoint_table) {
            continue;
        }
        if !tables.contains(&endpoint_table) {
            ctx.emit(surrealguard_diagnostics::catalog::finding(
                span,
                3006,
                format!(
                    "relation `{edge_name}` expects its `{side}` endpoint to be {}; found `{endpoint_table}`",
                    table_list(tables)
                ),
            ));
        }
    }
}

fn endpoint_table_name(endpoint: &ast::Spanned<ast::Expr>) -> Option<String> {
    match &endpoint.node {
        ast::Expr::Table(name) => Some(name.node.clone()),
        ast::Expr::RecordId { table, .. } => Some(table.node.clone()),
        _ => None,
    }
}

fn table_list(tables: &[String]) -> String {
    tables
        .iter()
        .map(|t| format!("`{t}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SchemaIndex;
    use crate::statement_env::StatementEnv;
    use surrealguard_syntax::lower::lower_statement;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    use crate::schema::extract_schema;

    fn analyze(schema: &SchemaIndex, query: &str) -> Kind {
        let parsed = parse_source(SourceId::new("query"), query).expect("query should parse");
        let node = crate::analyzer::test_support::find_first_node(
            parsed.tree().root_node(),
            "RelateStatement",
        )
        .expect("relate statement exists");
        let ast::Statement::Relate(stmt) = lower_statement(node, parsed.text()).node else {
            panic!("expected relate statement");
        };
        let env = StatementEnv::default();
        let mut diagnostics: Vec<surrealguard_diagnostics::Finding> = Vec::new();
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
